//! Capture-metadata extraction for images **and** videos.
//!
//! Populates `storage.file_metadata.captured_at` (and, for images, GPS / camera
//! / orientation / dimensions) so the Photos timeline can group by the real
//! capture date instead of the upload time. A DB trigger
//! (`trg_sync_media_sort_date`) keeps `storage.files.media_sort_date =
//! COALESCE(captured_at, created_at)` in sync, so writing `captured_at` is all
//! that is needed — the query/DTO/frontend already consume it.
//!
//! Mirrors [`AudioMetadataService`](super::audio_metadata_service): it is a
//! [`FileLifecycleHook`] wired into the upload pipeline, runs extraction off the
//! Tokio workers (`spawn_blocking`), and is dedup/copy aware.
//!
//! - **Images** — rich EXIF via the existing [`ExifService`] (kamadak-exif:
//!   GPS, camera, orientation, dimensions, capture date). The capture date is
//!   then upgraded to a timezone-correct value via `nom-exif`, which parses
//!   `OffsetTimeOriginal` (falling back to the kamadak naive value).
//! - **Videos** — container creation time (`mov`/`mp4`/`mkv`) via `nom-exif`,
//!   which seeks the metadata atoms (never loads the whole file) and is
//!   timezone-aware. No EXIF exists for video.

use chrono::{DateTime, FixedOffset, Utc};
use futures::StreamExt;
use sqlx::{FromRow, PgPool};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{info, warn};
use uuid::Uuid;

use crate::application::ports::file_lifecycle::FileLifecycleHook;
use crate::common::errors::DomainError;
use crate::infrastructure::repositories::pg::file_metadata_repository::FileMetadataRepository;
use crate::infrastructure::services::exif_service::{ExifMetadata, ExifService};

#[derive(Debug, FromRow)]
pub struct MediaFileRow {
    pub file_id: Uuid,
    pub blob_hash: String,
    pub mime_type: String,
}

#[derive(Debug, serde::Serialize)]
pub struct MetadataExtractionResult {
    pub total: usize,
    pub processed: usize,
    pub failed: usize,
}

pub struct MediaMetadataService {
    pool: Arc<PgPool>,
    blob_root: PathBuf,
}

impl MediaMetadataService {
    pub fn new(pool: Arc<PgPool>, blob_root: PathBuf) -> Self {
        Self { pool, blob_root }
    }

    pub fn is_image_file(mime_type: &str) -> bool {
        mime_type.starts_with("image/")
    }

    pub fn is_video_file(mime_type: &str) -> bool {
        mime_type.starts_with("video/")
    }

    /// Whether this service extracts capture metadata for the given MIME type.
    pub fn handles(mime_type: &str) -> bool {
        Self::is_image_file(mime_type) || Self::is_video_file(mime_type)
    }

    fn blob_path(&self, hash: &str) -> PathBuf {
        let prefix = &hash[0..2];
        self.blob_root.join(prefix).join(format!("{}.blob", hash))
    }

    fn arc(&self) -> Arc<Self> {
        Arc::new(Self {
            pool: self.pool.clone(),
            blob_root: self.blob_root.clone(),
        })
    }

    /// Extract capture metadata from a media blob.
    ///
    /// All parsing is synchronous (kamadak-exif + nom-exif), so this MUST only
    /// be called inside `spawn_blocking`. Returns `None` when there is nothing
    /// worth persisting (no EXIF and no capture date) — the caller then skips
    /// the upsert and the file legitimately falls back to its upload date.
    fn extract_blocking(path: &Path, mime_type: &str) -> Option<ExifMetadata> {
        if !path.exists() {
            warn!("Media file does not exist: {:?}", path);
            return None;
        }

        if Self::is_image_file(mime_type) {
            // Rich EXIF (GPS / camera / orientation / dimensions + naive date)
            // from the proven kamadak extractor.
            let mut meta = std::fs::read(path)
                .ok()
                .and_then(|b| ExifService::extract(&b));
            // Upgrade the capture date to a timezone-correct instant when the
            // image carries OffsetTimeOriginal (nom-exif). Falls back to the
            // kamadak naive value otherwise.
            let tz_date = capture_date(path);
            match (meta.as_mut(), tz_date) {
                (Some(m), Some(dt)) => m.captured_at = Some(dt),
                (Some(_), None) => { /* keep kamadak's naive captured_at */ }
                (None, Some(dt)) => {
                    meta = Some(ExifMetadata {
                        captured_at: Some(dt),
                        ..Default::default()
                    });
                }
                (None, None) => {}
            }
            meta
        } else if Self::is_video_file(mime_type) {
            // Videos carry no EXIF — pull the container creation time only.
            capture_date(path).map(|dt| ExifMetadata {
                captured_at: Some(dt),
                ..Default::default()
            })
        } else {
            None
        }
    }

    /// Extract metadata for one file and persist it (no-op when nothing useful
    /// could be extracted).
    pub async fn extract_and_save(
        &self,
        file_id: &Uuid,
        file_path: &Path,
        mime_type: &str,
    ) -> Result<(), DomainError> {
        let path = file_path.to_path_buf();
        let mime = mime_type.to_string();
        let meta = tokio::task::spawn_blocking(move || Self::extract_blocking(&path, &mime))
            .await
            .map_err(|e| {
                DomainError::internal_error(
                    "MediaMetadataService",
                    format!("spawn_blocking join error: {e}"),
                )
            })?;

        let Some(meta) = meta else {
            return Ok(());
        };

        FileMetadataRepository::new(self.pool.clone())
            .upsert(&file_id.to_string(), &meta)
            .await?;
        info!(
            "Saved capture metadata for file {} (captured_at={:?})",
            file_id, meta.captured_at
        );
        Ok(())
    }

    pub async fn delete_metadata(&self, file_id: &Uuid) -> Result<(), DomainError> {
        sqlx::query("DELETE FROM storage.file_metadata WHERE file_id = $1")
            .bind(file_id)
            .execute(&*self.pool)
            .await
            .map_err(|e| {
                DomainError::database_error(format!("Failed to delete file metadata: {}", e))
            })?;
        Ok(())
    }

    pub fn spawn_extraction_background(
        service: Arc<Self>,
        file_id: Uuid,
        file_path: PathBuf,
        mime_type: String,
    ) {
        tokio::spawn(async move {
            tracing::info!("📷 Extracting capture metadata for: {}", file_id);
            if let Err(e) = service
                .extract_and_save(&file_id, &file_path, &mime_type)
                .await
            {
                tracing::warn!("Failed to extract capture metadata: {}", e);
            }
        });
    }

    pub fn spawn_extraction_with_delete_background(
        service: Arc<Self>,
        file_id: Uuid,
        file_path: PathBuf,
        mime_type: String,
    ) {
        tokio::spawn(async move {
            tracing::info!("📷 Updating capture metadata for: {}", file_id);
            let _ = service.delete_metadata(&file_id).await;
            if let Err(e) = service
                .extract_and_save(&file_id, &file_path, &mime_type)
                .await
            {
                tracing::warn!("Failed to update capture metadata: {}", e);
            }
        });
    }

    /// Clone metadata from a known source file (explicit copy); falls back to a
    /// blob-hash lookup or fresh extraction if the source is not yet processed.
    pub fn clone_from_source_background(
        service: Arc<Self>,
        new_file_id: Uuid,
        source_file_id: Uuid,
        blob_hash: String,
        mime_type: String,
    ) {
        tokio::spawn(async move {
            let result = sqlx::query(
                r#"
                INSERT INTO storage.file_metadata
                    (file_id, captured_at, latitude, longitude, camera_make,
                     camera_model, orientation, width, height)
                SELECT $1, captured_at, latitude, longitude, camera_make,
                       camera_model, orientation, width, height
                FROM storage.file_metadata
                WHERE file_id = $2
                ON CONFLICT (file_id) DO NOTHING
                "#,
            )
            .bind(new_file_id)
            .bind(source_file_id)
            .execute(&*service.pool)
            .await;

            match result {
                Ok(r) if r.rows_affected() > 0 => {
                    info!(
                        "Cloned capture metadata from {} to {}",
                        source_file_id, new_file_id
                    );
                }
                Ok(_) => {
                    Self::clone_or_extract_background(service, new_file_id, blob_hash, mime_type);
                }
                Err(e) => {
                    warn!(
                        "Failed to clone capture metadata from {} to {}: {}",
                        source_file_id, new_file_id, e
                    );
                }
            }
        });
    }

    /// Clone metadata from any file sharing the same blob; falls back to fresh
    /// extraction if no processed sibling exists yet.
    pub fn clone_or_extract_background(
        service: Arc<Self>,
        new_file_id: Uuid,
        blob_hash: String,
        mime_type: String,
    ) {
        tokio::spawn(async move {
            let rows_inserted = sqlx::query(
                r#"
                INSERT INTO storage.file_metadata
                    (file_id, captured_at, latitude, longitude, camera_make,
                     camera_model, orientation, width, height)
                SELECT $1, fm.captured_at, fm.latitude, fm.longitude, fm.camera_make,
                       fm.camera_model, fm.orientation, fm.width, fm.height
                FROM storage.file_metadata fm
                JOIN storage.files sf ON sf.id = fm.file_id
                WHERE sf.blob_hash = $2
                LIMIT 1
                ON CONFLICT (file_id) DO NOTHING
                "#,
            )
            .bind(new_file_id)
            .bind(&blob_hash)
            .execute(&*service.pool)
            .await;

            match rows_inserted {
                Ok(result) if result.rows_affected() > 0 => {
                    info!("Cloned capture metadata for file {}", new_file_id);
                }
                Ok(_) => {
                    let file_path = service.blob_path(&blob_hash);
                    if let Err(e) = service
                        .extract_and_save(&new_file_id, &file_path, &mime_type)
                        .await
                    {
                        warn!(
                            "Failed to extract capture metadata for {}: {}",
                            new_file_id, e
                        );
                    }
                }
                Err(e) => {
                    warn!(
                        "Failed to clone capture metadata for {}: {}",
                        new_file_id, e
                    );
                }
            }
        });
    }

    /// Backfill: re-extract capture metadata for every existing image/video.
    /// Streams rows (O(1) memory); failures are logged and skipped, never abort
    /// the batch. Each upsert fires the DB trigger that recomputes
    /// `media_sort_date`, so the Photos timeline re-buckets afterwards.
    pub async fn reextract_all_image_metadata(
        &self,
    ) -> Result<MetadataExtractionResult, DomainError> {
        let mut stream = sqlx::query_as::<_, MediaFileRow>(
            r#"
            SELECT id as file_id, blob_hash, mime_type
            FROM storage.files
            WHERE mime_type LIKE 'image/%' OR mime_type LIKE 'video/%'
            "#,
        )
        .fetch(&*self.pool);

        let mut total: usize = 0;
        let mut processed: usize = 0;
        let mut failed: usize = 0;

        info!("Starting streaming capture-metadata backfill for image/video files");

        while let Some(row) = stream.next().await {
            total += 1;
            let media = row.map_err(|e| {
                DomainError::database_error(format!("Failed to fetch media file row: {}", e))
            })?;
            let file_path = self.blob_path(&media.blob_hash);
            match self
                .extract_and_save(&media.file_id, &file_path, &media.mime_type)
                .await
            {
                Ok(()) => processed += 1,
                Err(e) => {
                    warn!(
                        "Failed to extract capture metadata for file {}: {}",
                        media.file_id, e
                    );
                    failed += 1;
                }
            }
        }

        info!(
            "Capture-metadata backfill complete: {} processed, {} failed out of {} total",
            processed, failed, total
        );

        Ok(MetadataExtractionResult {
            total,
            processed,
            failed,
        })
    }
}

/// Extract a timezone-correct capture instant from an image (EXIF
/// `DateTimeOriginal`/`CreateDate`) or video/audio container (`CreateDate`).
///
/// `nom-exif` returns an offset-aware `DateTime<FixedOffset>` when the file
/// carries `OffsetTimeOriginal` (or a tz-aware container time); when it does
/// not, the naive wall-clock is interpreted as UTC. Either way the result is
/// converted to a true UTC instant. Returns `None` if no capture date exists.
fn capture_date(path: &Path) -> Option<DateTime<Utc>> {
    use nom_exif::{EntryValue, ExifTag, TrackInfoTag, read_exif, read_track};

    let to_utc = |ev: &EntryValue| -> Option<DateTime<Utc>> {
        let edt = ev.as_datetime()?;
        let utc0 = FixedOffset::east_opt(0)?;
        Some(edt.or_offset(utc0).with_timezone(&Utc))
    };

    // Images: EXIF DateTimeOriginal, then DateTimeDigitized (CreateDate).
    if let Ok(exif) = read_exif(path) {
        if let Some(dt) = exif.get(ExifTag::DateTimeOriginal).and_then(to_utc) {
            return Some(dt);
        }
        if let Some(dt) = exif.get(ExifTag::CreateDate).and_then(to_utc) {
            return Some(dt);
        }
    }

    // Videos / audio containers (mov/mp4/mkv): track creation time.
    if let Ok(track) = read_track(path)
        && let Some(dt) = track.get(TrackInfoTag::CreateDate).and_then(to_utc)
    {
        return Some(dt);
    }

    None
}

// ─── FileLifecycleHook ───────────────────────────────────────────────────────

impl FileLifecycleHook for MediaMetadataService {
    fn on_file_created(
        &self,
        file_id: &str,
        blob_hash: &str,
        content_type: &str,
        is_new_blob: bool,
    ) {
        if !Self::handles(content_type) {
            return;
        }
        let Ok(uuid) = file_id.parse::<Uuid>() else {
            warn!("on_file_created: invalid file_id UUID: {}", file_id);
            return;
        };
        let service = self.arc();
        if is_new_blob {
            Self::spawn_extraction_background(
                service,
                uuid,
                self.blob_path(blob_hash),
                content_type.to_string(),
            );
        } else {
            Self::clone_or_extract_background(
                service,
                uuid,
                blob_hash.to_string(),
                content_type.to_string(),
            );
        }
    }

    fn on_file_copied(
        &self,
        file_id: &str,
        blob_hash: &str,
        content_type: &str,
        source_file_id: &str,
    ) {
        if !Self::handles(content_type) {
            return;
        }
        let Ok(uuid) = file_id.parse::<Uuid>() else {
            warn!("on_file_copied: invalid file_id UUID: {}", file_id);
            return;
        };
        let Ok(source_uuid) = source_file_id.parse::<Uuid>() else {
            warn!(
                "on_file_copied: invalid source_file_id UUID: {}",
                source_file_id
            );
            return;
        };
        Self::clone_from_source_background(
            self.arc(),
            uuid,
            source_uuid,
            blob_hash.to_string(),
            content_type.to_string(),
        );
    }

    fn on_file_updated(&self, file_id: &str, blob_hash: &str, content_type: &str) {
        if !Self::handles(content_type) {
            return;
        }
        let Ok(uuid) = file_id.parse::<Uuid>() else {
            warn!("on_file_updated: invalid file_id UUID: {}", file_id);
            return;
        };
        Self::spawn_extraction_with_delete_background(
            self.arc(),
            uuid,
            self.blob_path(blob_hash),
            content_type.to_string(),
        );
    }

    fn on_file_deleted(&self, _file_id: &str) {
        // storage.file_metadata has ON DELETE CASCADE on file_id — DB handles cleanup.
    }
}
