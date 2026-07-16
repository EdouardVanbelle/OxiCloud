use crate::application::services::file_retrieval_service::FileRetrievalService;
use crate::application::services::folder_service::FolderService;
use crate::{
    application::dtos::file_dto::FileDto,
    application::ports::file_ports::FileRetrievalUseCase,
    application::ports::folder_ports::FolderUseCase,
    application::ports::zip_ports::ZipPort,
    common::errors::{DomainError, ErrorKind, Result},
};
use async_zip::base::write::ZipFileWriter;
use async_zip::{Compression, ZipEntryBuilder};
use futures::StreamExt;
use futures::io::AsyncWriteExt as FuturesWriteExt;
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::NamedTempFile;
use thiserror::Error;
use tokio::io::BufWriter;
use tokio_util::compat::Compat;
use tracing::*;

/// Error related to ZIP file creation
#[derive(Debug, Error)]
pub enum ZipError {
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("ZIP error: {0}")]
    AsyncZipError(#[from] async_zip::error::ZipError),

    #[error("Error reading file: {0}")]
    FileReadError(String),

    #[error("Error getting folder contents: {0}")]
    FolderContentsError(String),

    #[error("Folder not found: {0}")]
    FolderNotFound(String),
}

impl From<ZipError> for DomainError {
    fn from(err: ZipError) -> Self {
        DomainError::new(ErrorKind::InternalError, "zip_service", err.to_string())
    }
}

/// Fully-async ZIP writer over any buffered tokio sink (temp file for the
/// legacy path, one half of a `tokio::io::duplex` for the streaming path).
type AsyncZipWriter<W> = ZipFileWriter<Compat<BufWriter<W>>>;

/// One planned archive entry, in final ZIP order.
enum ZipPlanEntry {
    /// Directory entry (Stored, zero-length body).
    Dir(String),
    /// File entry: ZIP-relative path + file id to stream from the blob store.
    /// `compression` is picked from the file's MIME type at plan time —
    /// `Stored` for already-compressed media (JPEG/MP4/…), `Deflate` otherwise.
    File {
        zip_path: String,
        file_id: String,
        compression: Compression,
    },
}

/// Message protocol from the prefetch task to the ZIP writer. For each
/// planned file, in order: zero or more `Chunk`s, then exactly one `End`;
/// `Err` aborts the whole archive.
enum Prefetched {
    Chunk(bytes::Bytes),
    End,
    Err(String),
}

/// Bound on the prefetch channel (messages of ≤ ~64 KB blob-stream chunks):
/// ~4 MiB of read-ahead. Enough to hide the per-file open latency of the
/// blob store (PG lookup + backend round-trip — significant on S3/Azure)
/// behind the deflate of the previous entry, while keeping RAM flat.
const PREFETCH_BUFFER_CHUNKS: usize = 64;

/// Service for creating ZIP files.
///
/// Uses `async_zip` for fully-async archive creation.  Every write (headers,
/// compressed chunk data, central directory) goes through
/// `tokio::io::BufWriter` → `tokio::fs::File`, so no Tokio worker is ever
/// blocked by disk I/O. Deflate itself DOES run inline on the writing task
/// (async_zip compresses inside `poll_write`), which is why entries whose
/// MIME says the content is already compressed are `Stored` instead — that
/// turns the archive hot path from ~1 CPU core per download into CRC + memcpy.
///
/// Archive creation is a 2-stage pipeline: a prefetch task reads file
/// content from the blob store ahead of the writer, so the next file's
/// read latency overlaps the current file's compression instead of adding
/// to it. The ZIP entries themselves are still written strictly in order
/// (the format requires it).
pub struct ZipService {
    file_service: Arc<FileRetrievalService>,
    folder_service: Arc<FolderService>,
}

impl ZipService {
    /// Creates a new instance of the ZIP service
    pub fn new(
        file_service: Arc<FileRetrievalService>,
        folder_service: Arc<FolderService>,
    ) -> Self {
        Self {
            file_service,
            folder_service,
        }
    }

    /// Creates a ZIP file backed by a temporary file, containing the contents
    /// of a folder and all its subfolders.  Returns the `NamedTempFile` so the
    /// caller can stream it and let the OS clean up on drop.
    ///
    /// Uses **2 SQL queries** (ltree `<@`) to fetch the entire subtree instead
    /// of the previous N+1 BFS traversal.
    pub async fn create_folder_zip(
        &self,
        folder_id: &str,
        folder_name: &str,
    ) -> Result<NamedTempFile> {
        let plan = self.plan_archive(folder_id, folder_name).await?;

        // ── Open the temp file + ZIP writer ──────────────────────────────
        let temp = NamedTempFile::new().map_err(ZipError::IoError)?;
        let tokio_file = tokio::fs::File::create(temp.path())
            .await
            .map_err(ZipError::IoError)?;

        let (tx, mut rx) = tokio::sync::mpsc::channel::<Prefetched>(PREFETCH_BUFFER_CHUNKS);
        let _prefetcher = tokio::spawn(Self::prefetch_files(
            self.file_service.clone(),
            Self::planned_file_ids(&plan),
            tx,
        ));
        Self::write_archive(tokio_file, &plan, &mut rx).await?;

        Ok(temp)
    }

    /// Streaming variant: the archive bytes are produced on a spawned task
    /// and yielded as they are written — the client's first byte arrives
    /// after the first entry starts, not after the whole archive has been
    /// built (the temp-file variant's time-to-first-byte grows with folder
    /// size; benches/ZIP-STREAM.md). The plan phase still runs inline so
    /// planning errors surface as proper HTTP errors; a blob-read error
    /// mid-archive can only truncate the stream (no central directory →
    /// clients detect the corrupt archive), which is the standard tradeoff
    /// for streamed ZIPs.
    pub async fn create_folder_zip_stream(
        &self,
        folder_id: &str,
        folder_name: &str,
    ) -> Result<impl futures::Stream<Item = std::io::Result<bytes::Bytes>> + Send + use<>> {
        let plan = self.plan_archive(folder_id, folder_name).await?;

        let (writer, reader) = tokio::io::duplex(256 * 1024);
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Prefetched>(PREFETCH_BUFFER_CHUNKS);
        let _prefetcher = tokio::spawn(Self::prefetch_files(
            self.file_service.clone(),
            Self::planned_file_ids(&plan),
            tx,
        ));
        tokio::spawn(async move {
            if let Err(e) = Self::write_archive(writer, &plan, &mut rx).await {
                // Dropping the writer EOFs the reader early — the truncated
                // archive has no central directory, so clients flag it.
                warn!("Streaming ZIP aborted mid-archive: {e}");
            }
        });

        Ok(tokio_util::io::ReaderStream::new(reader))
    }

    /// File ids of the plan, in archive order (the prefetcher's read list).
    fn planned_file_ids(plan: &[ZipPlanEntry]) -> Vec<String> {
        plan.iter()
            .filter_map(|entry| match entry {
                ZipPlanEntry::File { file_id, .. } => Some(file_id.clone()),
                ZipPlanEntry::Dir(_) => None,
            })
            .collect()
    }

    /// Write every planned entry through a buffered ZIP writer over `sink`,
    /// then finalize (central directory + flush). Shared by the temp-file
    /// and streaming variants.
    async fn write_archive<W: tokio::io::AsyncWrite + Unpin>(
        sink: W,
        plan: &[ZipPlanEntry],
        rx: &mut tokio::sync::mpsc::Receiver<Prefetched>,
    ) -> Result<()> {
        let buf_writer = BufWriter::with_capacity(256 * 1024, sink);
        let mut zip = ZipFileWriter::with_tokio(buf_writer);

        for entry in plan {
            match entry {
                ZipPlanEntry::Dir(zip_dir) => {
                    let dir_entry =
                        ZipEntryBuilder::new(zip_dir.clone().into(), Compression::Stored);
                    match zip.write_entry_whole(dir_entry, &[]).await {
                        Ok(()) => debug!("Folder added to ZIP: {}", zip_dir),
                        Err(e) => {
                            warn!("Could not add folder entry (may already exist): {}", e);
                        }
                    }
                }
                ZipPlanEntry::File {
                    zip_path,
                    compression,
                    ..
                } => {
                    Self::write_prefetched_file(&mut zip, zip_path, *compression, rx).await?;
                }
            }
        }

        let mut compat_writer = zip.close().await.map_err(ZipError::AsyncZipError)?;
        compat_writer.close().await.map_err(ZipError::IoError)?;
        Ok(())
    }

    /// Resolve the folder, fetch its subtree (2 bulk queries) and lay out
    /// the archive entries in final ZIP order.
    async fn plan_archive(&self, folder_id: &str, folder_name: &str) -> Result<Vec<ZipPlanEntry>> {
        info!(
            "Creating ZIP for folder: {} (ID: {})",
            folder_name, folder_id
        );

        // Verify the folder exists and get its path for prefix stripping
        let root_folder = match self.folder_service.get_folder(folder_id).await {
            Ok(f) => f,
            Err(e) => {
                error!("Error getting folder {}: {}", folder_id, e);
                return Err(ZipError::FolderNotFound(folder_id.to_string()).into());
            }
        };

        // ── 1. Bulk-fetch folder tree (small — one entry per folder) ────
        let all_folders = self
            .folder_service
            .list_subtree_folders(folder_id)
            .await
            .map_err(|e| ZipError::FolderContentsError(format!("subtree folders: {}", e)))?;

        // ── 2. Stream files from DB cursor — O(1) per row ───────────────
        let mut file_stream = self
            .file_service
            .stream_files_in_subtree(folder_id)
            .await
            .map_err(|e| ZipError::FolderContentsError(format!("subtree files: {}", e)))?;

        // Group files by folder_id incrementally from the stream
        let mut files_by_folder: HashMap<String, Vec<FileDto>> =
            HashMap::with_capacity(all_folders.len());
        while let Some(file) = file_stream.next().await {
            let file =
                file.map_err(|e| ZipError::FolderContentsError(format!("subtree file: {}", e)))?;
            let fid = file.folder_id.clone().unwrap_or_default();
            files_by_folder.entry(fid).or_default().push(file);
        }

        info!(
            "ZIP subtree: {} folders, {} files",
            all_folders.len(),
            files_by_folder.values().map(|v| v.len()).sum::<usize>()
        );

        // ── 3. Build a mapping: folder_id → ZIP-relative path ────────────
        //
        // The root folder's DB path is e.g. "/users/alice/Documents".
        // We want ZIP entries relative to `folder_name`, so we strip the
        // root prefix and prepend `folder_name`.
        let root_path = root_folder.path.trim_end_matches('/');
        let folder_zip_path = |db_path: &str| -> String {
            let db_path = db_path.trim_end_matches('/');
            if db_path == root_path {
                folder_name.to_string()
            } else {
                let suffix = db_path
                    .strip_prefix(root_path)
                    .unwrap_or(db_path)
                    .trim_start_matches('/');
                format!("{}/{}", folder_name, suffix)
            }
        };

        // ── 4. Plan the archive (folders are already sorted by path) ─────
        let mut plan: Vec<ZipPlanEntry> = Vec::new();
        for folder in &all_folders {
            let zip_dir = format!("{}/", folder_zip_path(&folder.path));
            plan.push(ZipPlanEntry::Dir(zip_dir.clone()));
            if let Some(files) = files_by_folder.get(&folder.id) {
                for file in files {
                    plan.push(ZipPlanEntry::File {
                        zip_path: format!("{}{}", zip_dir, file.name),
                        file_id: file.id.to_string(),
                        compression: crate::common::mime_detect::zip_entry_compression(
                            &file.mime_type,
                        ),
                    });
                }
            }
        }

        Ok(plan)
    }

    /// Prefetch stage: streams each planned file's content from the blob
    /// store, in plan order, into the bounded channel. Stops on the first
    /// read error (after forwarding it) or when the writer hangs up.
    async fn prefetch_files(
        file_service: Arc<FileRetrievalService>,
        file_ids: Vec<String>,
        tx: tokio::sync::mpsc::Sender<Prefetched>,
    ) {
        for file_id in file_ids {
            let stream = match file_service.get_file_stream(&file_id).await {
                Ok(s) => s,
                Err(e) => {
                    error!("Error opening file stream {}: {}", file_id, e);
                    let _ = tx
                        .send(Prefetched::Err(format!(
                            "Error streaming file {}: {}",
                            file_id, e
                        )))
                        .await;
                    return;
                }
            };

            let mut stream = std::pin::Pin::from(stream);
            while let Some(chunk_result) = stream.next().await {
                let message = match chunk_result {
                    Ok(bytes) => Prefetched::Chunk(bytes),
                    Err(e) => Prefetched::Err(format!("Error streaming file {}: {}", file_id, e)),
                };
                let abort = matches!(message, Prefetched::Err(_));
                if tx.send(message).await.is_err() || abort {
                    return; // writer gone, or fatal read error forwarded
                }
            }

            if tx.send(Prefetched::End).await.is_err() {
                return; // writer gone
            }
        }
    }

    /// Writer stage: drains one file's prefetched chunks into a ZIP entry
    /// (`Stored` for already-compressed media, `Deflate` otherwise — see
    /// `entry_compression`). Peak memory stays bounded by the channel,
    /// independent of individual file sizes.
    async fn write_prefetched_file<W: tokio::io::AsyncWrite + Unpin>(
        zip: &mut AsyncZipWriter<W>,
        zip_path: &str,
        compression: Compression,
        rx: &mut tokio::sync::mpsc::Receiver<Prefetched>,
    ) -> Result<()> {
        info!("Adding file to ZIP: {}", zip_path);

        let entry = ZipEntryBuilder::new(zip_path.to_string().into(), compression);
        let mut entry_writer = zip
            .write_entry_stream(entry)
            .await
            .map_err(ZipError::AsyncZipError)?;

        loop {
            match rx.recv().await {
                Some(Prefetched::Chunk(bytes)) => {
                    entry_writer
                        .write_all(&bytes)
                        .await
                        .map_err(ZipError::IoError)?;
                }
                Some(Prefetched::End) => break,
                Some(Prefetched::Err(message)) => {
                    // Close the partially-written entry before bailing out.
                    let _ = entry_writer.close().await;
                    return Err(ZipError::FileReadError(message).into());
                }
                None => {
                    let _ = entry_writer.close().await;
                    return Err(ZipError::FileReadError(format!(
                        "Prefetch stage ended unexpectedly while writing {}",
                        zip_path
                    ))
                    .into());
                }
            }
        }

        // Finalize the entry (writes data descriptor with CRC + sizes)
        entry_writer
            .close()
            .await
            .map_err(ZipError::AsyncZipError)?;

        debug!("File added to ZIP: {}", zip_path);
        Ok(())
    }
}

// ─── Port implementation ─────────────────────────────────────────────────────

impl ZipPort for ZipService {
    async fn create_folder_zip(
        &self,
        folder_id: &str,
        folder_name: &str,
    ) -> std::result::Result<NamedTempFile, DomainError> {
        self.create_folder_zip(folder_id, folder_name).await
    }
}
