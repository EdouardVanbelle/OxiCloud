//! Face indexing as a `FileLifecycleHook`.
//!
//! On image upload it detects + embeds faces (off the request path, in a
//! background task) and stores them. It mirrors `MediaMetadataService`: reads
//! the blob from the local `.blobs` tree, is dedup-aware (identical uploads
//! clone an existing file's faces instead of re-running inference), and is
//! completely inert when no model is configured (`FaceAnalyzerPort::is_ready()
//! == false`) — so the feature compiles and runs with the default no-op
//! analyzer until the operator wires a real ONNX model.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::Utc;
use sqlx::PgPool;
use uuid::Uuid;

use crate::application::ports::face_ports::{FaceAnalyzerPort, FaceRepository};
use crate::application::ports::file_lifecycle::FileLifecycleHook;
use crate::common::errors::DomainError;
use crate::domain::entities::face::Face;
use crate::infrastructure::repositories::pg::FacePgRepository;

/// Minimum detector confidence for a face to be stored.
const MIN_DET_SCORE: f32 = 0.6;

fn is_image(content_type: &str) -> bool {
    content_type.starts_with("image/")
}

/// Concurrent index-task budget. Env override
/// `OXICLOUD_FACES_INDEX_CONCURRENCY`, else the effective core count —
/// each task is a full-image read + decode + ONNX inference, so more
/// permits than cores only adds RAM pressure, not throughput.
fn max_concurrent_index() -> usize {
    std::env::var("OXICLOUD_FACES_INDEX_CONCURRENCY")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&n: &usize| n > 0)
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(2)
        })
}

pub struct FaceIndexingService {
    pool: Arc<PgPool>,
    repo: Arc<FacePgRepository>,
    analyzer: Arc<dyn FaceAnalyzerPort>,
    blob_root: PathBuf,
    /// Bounds concurrent indexing tasks. The lifecycle hooks spawn one
    /// task per uploaded/copied image with no ceiling, so a bulk upload
    /// used to fan out N simultaneous full-image reads + decodes +
    /// inferences — peak RSS N × image size plus CPU thrash. Same
    /// invariant as `ThumbnailService::decode_semaphore`: the permit is
    /// acquired BEFORE the blob read, so peak memory is
    /// `permits × image size` regardless of upload concurrency.
    index_semaphore: Arc<tokio::sync::Semaphore>,
}

impl FaceIndexingService {
    pub fn new(pool: Arc<PgPool>, blob_root: PathBuf, analyzer: Arc<dyn FaceAnalyzerPort>) -> Self {
        let repo = Arc::new(FacePgRepository::new(pool.clone()));
        Self {
            pool,
            repo,
            analyzer,
            blob_root,
            index_semaphore: Arc::new(tokio::sync::Semaphore::new(max_concurrent_index())),
        }
    }

    /// Local path of a blob: `.blobs/{prefix}/{hash}.blob`.
    fn blob_path(&self, hash: &str) -> PathBuf {
        let prefix = if hash.len() >= 2 { &hash[0..2] } else { hash };
        self.blob_root.join(prefix).join(format!("{hash}.blob"))
    }

    /// Spawn a background indexing task. `reuse_dedup` clones faces from an
    /// existing file with the same blob hash instead of re-running inference;
    /// `delete_first` clears prior faces (used on overwrite).
    fn spawn_index(&self, file_id: Uuid, blob_hash: String, reuse_dedup: bool, delete_first: bool) {
        let pool = self.pool.clone();
        let repo = self.repo.clone();
        let analyzer = self.analyzer.clone();
        let blob_path = self.blob_path(&blob_hash);
        let semaphore = self.index_semaphore.clone();
        tokio::spawn(async move {
            // Queue behind the concurrency budget BEFORE touching the
            // blob — excess tasks wait holding only this tiny future,
            // not a decoded image.
            let _permit = semaphore
                .acquire_owned()
                .await
                .expect("face index semaphore never closes");
            if delete_first {
                let _ = repo.delete_faces_for_file(file_id).await;
            }
            if let Err(e) = index_file(
                &pool,
                &repo,
                analyzer.as_ref(),
                file_id,
                &blob_path,
                &blob_hash,
                reuse_dedup,
            )
            .await
            {
                tracing::warn!(target: "oxicloud::faces", "face indexing failed for {file_id}: {e}");
            }
        });
    }
}

impl FileLifecycleHook for FaceIndexingService {
    fn on_file_created(
        &self,
        file_id: &str,
        blob_hash: &str,
        content_type: &str,
        is_new_blob: bool,
    ) {
        if !is_image(content_type) || !self.analyzer.is_ready() {
            return;
        }
        if let Ok(fid) = file_id.parse::<Uuid>() {
            // Dedup hit (blob already existed) → clone an existing file's faces.
            self.spawn_index(fid, blob_hash.to_string(), !is_new_blob, false);
        }
    }

    fn on_file_copied(
        &self,
        file_id: &str,
        blob_hash: &str,
        content_type: &str,
        _source_file_id: &str,
    ) {
        if !is_image(content_type) || !self.analyzer.is_ready() {
            return;
        }
        if let Ok(fid) = file_id.parse::<Uuid>() {
            self.spawn_index(fid, blob_hash.to_string(), true, false);
        }
    }

    fn on_file_updated(&self, file_id: &str, blob_hash: &str, content_type: &str) {
        if !is_image(content_type) || !self.analyzer.is_ready() {
            return;
        }
        if let Ok(fid) = file_id.parse::<Uuid>() {
            self.spawn_index(fid, blob_hash.to_string(), false, true);
        }
    }

    fn on_file_deleted(&self, _file_id: &str) {
        // faces.faces.file_id has ON DELETE CASCADE — the DB cleans up.
    }
}

async fn lookup_user(pool: &PgPool, file_id: Uuid) -> Result<Uuid, DomainError> {
    let row: (Uuid,) = sqlx::query_as("SELECT user_id FROM storage.files WHERE id = $1")
        .bind(file_id)
        .fetch_one(pool)
        .await
        .map_err(|e| DomainError::internal_error("Faces", format!("lookup user: {e}")))?;
    Ok(row.0)
}

async fn index_file(
    pool: &PgPool,
    repo: &FacePgRepository,
    analyzer: &dyn FaceAnalyzerPort,
    file_id: Uuid,
    blob_path: &Path,
    blob_hash: &str,
    reuse_dedup: bool,
) -> Result<(), DomainError> {
    let user_id = lookup_user(pool, file_id).await?;

    // Dedup-aware fast path: reuse faces already computed for an identical blob.
    if reuse_dedup {
        let peers = repo.faces_for_blob(user_id, blob_hash).await?;
        let cloned: Vec<Face> = peers
            .into_iter()
            .filter(|f| f.file_id != file_id)
            .map(|f| Face {
                id: Uuid::new_v4(),
                file_id,
                ..f
            })
            .collect();
        if !cloned.is_empty() {
            repo.save_faces(&cloned).await?;
            return Ok(());
        }
        // No peer found — fall through and analyze.
    }

    let bytes = tokio::fs::read(blob_path)
        .await
        .map_err(|e| DomainError::internal_error("Faces", format!("read blob: {e}")))?;
    let detected = analyzer.analyze(&bytes).await?;

    let faces: Vec<Face> = detected
        .into_iter()
        .filter(|d| d.det_score >= MIN_DET_SCORE)
        .map(|d| Face {
            id: Uuid::new_v4(),
            file_id,
            user_id,
            person_id: None,
            bbox: d.bbox,
            det_score: d.det_score,
            quality: d.quality,
            embedding: d.embedding,
            blob_hash: Some(blob_hash.to_string()),
            created_at: Utc::now(),
        })
        .collect();
    repo.save_faces(&faces).await
}
