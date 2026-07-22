//! `local_fs` external mount provider — serves a raw host filesystem directory.
//!
//! Bound to one canonicalized `host_path` at construction. `node_id` is the POSIX
//! path relative to that root (so `resolve_path` is identity and ids are NOT
//! stable across renames). Every op funnels through [`LocalFsMountProvider::resolve_existing`]
//! / [`LocalFsMountProvider::resolve_parent`], which reject path traversal and
//! symlink escape by canonicalizing and asserting the result stays under the
//! bound root.

use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio_util::io::ReaderStream;

use crate::application::ports::blob_storage_ports::BlobStream;
use crate::application::ports::external_mount_ports::{
    ExternalMountProvider, MountByteStream, MountCaps, MountEntry, MountStat,
};
use crate::domain::errors::DomainError;
use crate::domain::services::external_mount_id::NodeId;
use crate::domain::services::path_service::validate_storage_name;

/// Read buffer size for streamed file reads (mirrors the blob backend).
const STREAM_CHUNK_SIZE: usize = 256 * 1024;

/// Directories larger than this refuse to list (bounded in-memory sort/cursor).
const MAX_DIR_ENTRIES: usize = 50_000;

/// A mount provider backed by a local filesystem directory.
pub struct LocalFsMountProvider {
    /// Canonical absolute path of the mount root; the containment anchor.
    host_path: PathBuf,
    /// Whether mutations are refused.
    read_only: bool,
}

impl LocalFsMountProvider {
    /// Construct from a host directory path. The path must exist and be a
    /// directory; it is canonicalized once so symlink-containment checks are
    /// cheap thereafter.
    pub fn new(path: impl AsRef<Path>, read_only: bool) -> Result<Self, DomainError> {
        let host_path = std::fs::canonicalize(path.as_ref()).map_err(|e| {
            DomainError::internal_error(
                "ExternalMount",
                format!(
                    "mount host path {} is not accessible: {e}",
                    path.as_ref().display()
                ),
            )
        })?;
        if !host_path.is_dir() {
            return Err(DomainError::internal_error(
                "ExternalMount",
                format!("mount host path {} is not a directory", host_path.display()),
            ));
        }
        Ok(Self {
            host_path,
            read_only,
        })
    }

    /// Lexically join a relative node id onto the host root, rejecting any
    /// traversal, absolute, or otherwise unsafe component. Does NOT touch disk.
    fn lexical_path(&self, relpath: &str) -> Result<PathBuf, DomainError> {
        if relpath.is_empty() {
            return Ok(self.host_path.clone());
        }
        if relpath.starts_with('/') || relpath.contains('\0') || relpath.contains('\\') {
            return Err(DomainError::not_found("ExternalMount", relpath));
        }
        let mut out = self.host_path.clone();
        for segment in relpath.split('/') {
            // Reject empty / "." / ".." and re-use the storage-name validator
            // (no slashes, no null, not "."/"..").
            if validate_storage_name(segment).is_err() {
                return Err(DomainError::not_found("ExternalMount", relpath));
            }
            out.push(segment);
        }
        Ok(out)
    }

    /// Assert `candidate` (once canonicalized) stays under the host root.
    /// Returns the canonical path to use for the actual op (so we never follow
    /// an escaping symlink).
    async fn assert_within(&self, candidate: &Path, relpath: &str) -> Result<PathBuf, DomainError> {
        let canonical = tokio::fs::canonicalize(candidate)
            .await
            .map_err(|e| map_io_err(e, relpath))?;
        if !canonical.starts_with(&self.host_path) {
            // Symlink escape or traversal — treat as not found (anti-enumeration).
            return Err(DomainError::not_found("ExternalMount", relpath));
        }
        Ok(canonical)
    }

    /// Resolve an existing entry to its canonical on-disk path.
    async fn resolve_existing(&self, node: &NodeId) -> Result<PathBuf, DomainError> {
        let lexical = self.lexical_path(node.as_str())?;
        self.assert_within(&lexical, node.as_str()).await
    }

    /// Resolve a parent directory (must exist) to its canonical path, for
    /// create/write ops whose final target does not exist yet.
    async fn resolve_parent(&self, parent: &NodeId) -> Result<PathBuf, DomainError> {
        let lexical = self.lexical_path(parent.as_str())?;
        let canonical = self.assert_within(&lexical, parent.as_str()).await?;
        if !canonical.is_dir() {
            return Err(DomainError::not_found("ExternalMount", parent.as_str()));
        }
        Ok(canonical)
    }

    /// Refuse mutations on read-only mounts.
    fn ensure_writable(&self) -> Result<(), DomainError> {
        if self.read_only {
            return Err(DomainError::operation_not_supported(
                "ExternalMount",
                "mount is read-only",
            ));
        }
        Ok(())
    }

    /// Refuse operations that target the mount root itself (empty node id).
    ///
    /// Without this, `delete("")` would `remove_dir_all` the entire mount root
    /// and `rename("", …)` would move the root *outside* the containment anchor
    /// (its parent directory lives outside the mount). The mount root is managed
    /// as a `storage.folders` row, never through the provider.
    fn ensure_not_root(node: &NodeId) -> Result<(), DomainError> {
        if node.as_str().is_empty() {
            return Err(DomainError::operation_not_supported(
                "ExternalMount",
                "the mount root itself cannot be modified through the provider",
            ));
        }
        Ok(())
    }

    /// Build a child node id given a parent relpath and child name.
    fn child_node_id(parent_relpath: &str, name: &str) -> NodeId {
        if parent_relpath.is_empty() {
            NodeId(name.to_string())
        } else {
            NodeId(format!("{parent_relpath}/{name}"))
        }
    }

    /// Turn a path + its metadata into a [`MountStat`].
    fn stat_from_meta(node_id: NodeId, name: &str, meta: &std::fs::Metadata) -> MountStat {
        let is_dir = meta.is_dir();
        MountStat {
            node_id,
            is_dir,
            size: if is_dir { 0 } else { meta.len() },
            modified_at: system_time_secs(meta.modified().ok()),
            created_at: system_time_secs(meta.created().ok().or_else(|| meta.modified().ok())),
            mime_type: if is_dir {
                "directory".to_string()
            } else {
                mime_guess::from_path(name)
                    .first_or_octet_stream()
                    .to_string()
            },
        }
    }
}

/// Map a `std::io::Error` to a `DomainError`, preserving anti-enumeration for
/// not-found and a stable shape for the rest.
fn map_io_err(e: std::io::Error, relpath: &str) -> DomainError {
    use std::io::ErrorKind as Io;
    match e.kind() {
        Io::NotFound => DomainError::not_found("ExternalMount", relpath),
        Io::PermissionDenied => {
            DomainError::access_denied("ExternalMount", format!("permission denied: {relpath}"))
        }
        Io::AlreadyExists => DomainError::already_exists("ExternalMount", relpath),
        _ => DomainError::internal_error("ExternalMount", format!("io error on {relpath}: {e}")),
    }
}

/// Convert an optional `SystemTime` to unix seconds (0 when unavailable).
fn system_time_secs(t: Option<SystemTime>) -> u64 {
    t.and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Final path segment of a node id, for display + mime sniffing.
fn node_name(relpath: &str) -> &str {
    relpath.rsplit('/').next().unwrap_or(relpath)
}

#[async_trait]
impl ExternalMountProvider for LocalFsMountProvider {
    fn kind(&self) -> &'static str {
        "local_fs"
    }

    fn capabilities(&self) -> MountCaps {
        MountCaps {
            supports_range: true,
            read_only: self.read_only,
            stable_ids: false,
        }
    }

    // resolve_path uses the default identity impl (node_id == relpath).

    async fn list_dir(&self, node_id: &NodeId) -> Result<Vec<MountEntry>, DomainError> {
        let dir = self.resolve_existing(node_id).await?;
        let parent_rel = node_id.as_str();

        let mut read_dir = tokio::fs::read_dir(&dir)
            .await
            .map_err(|e| map_io_err(e, parent_rel))?;

        let mut entries = Vec::new();
        while let Some(dirent) = read_dir
            .next_entry()
            .await
            .map_err(|e| map_io_err(e, parent_rel))?
        {
            // Reject any non-UTF8 / unsafe name (cannot round-trip through ids).
            let os_name = dirent.file_name();
            let Some(name) = os_name.to_str() else {
                continue;
            };
            if validate_storage_name(name).is_err() {
                continue;
            }
            // Skip symlinks that escape the mount root (containment check).
            let meta = match dirent.metadata().await {
                Ok(m) => m,
                Err(_) => continue,
            };
            if meta.file_type().is_symlink() {
                let child_lex = dir.join(name);
                if self.assert_within(&child_lex, name).await.is_err() {
                    continue;
                }
            }
            let node_id = Self::child_node_id(parent_rel, name);
            let is_dir = meta.is_dir();
            entries.push(MountEntry {
                name: name.to_string(),
                node_id,
                is_dir,
                size: if is_dir { 0 } else { meta.len() },
                modified_at: system_time_secs(meta.modified().ok()),
                created_at: system_time_secs(meta.created().ok().or_else(|| meta.modified().ok())),
            });

            if entries.len() > MAX_DIR_ENTRIES {
                return Err(DomainError::operation_not_supported(
                    "ExternalMount",
                    format!("directory exceeds {MAX_DIR_ENTRIES} entries"),
                ));
            }
        }
        Ok(entries)
    }

    async fn stat(&self, node_id: &NodeId) -> Result<MountStat, DomainError> {
        let path = self.resolve_existing(node_id).await?;
        let meta = tokio::fs::metadata(&path)
            .await
            .map_err(|e| map_io_err(e, node_id.as_str()))?;
        Ok(Self::stat_from_meta(
            node_id.clone(),
            node_name(node_id.as_str()),
            &meta,
        ))
    }

    async fn open_read_stream(
        &self,
        node_id: &NodeId,
        range: Option<(u64, Option<u64>)>,
    ) -> Result<BlobStream, DomainError> {
        let path = self.resolve_existing(node_id).await?;
        let mut file = tokio::fs::File::open(&path)
            .await
            .map_err(|e| map_io_err(e, node_id.as_str()))?;

        match range {
            Some((start, end_inclusive)) => {
                file.seek(SeekFrom::Start(start))
                    .await
                    .map_err(|e| map_io_err(e, node_id.as_str()))?;
                if let Some(end) = end_inclusive {
                    // HTTP range end is inclusive.
                    let limit = end.saturating_sub(start).saturating_add(1);
                    let limited = file.take(limit);
                    Ok(
                        Box::pin(ReaderStream::with_capacity(limited, STREAM_CHUNK_SIZE))
                            as BlobStream,
                    )
                } else {
                    Ok(
                        Box::pin(ReaderStream::with_capacity(file, STREAM_CHUNK_SIZE))
                            as BlobStream,
                    )
                }
            }
            None => {
                Ok(Box::pin(ReaderStream::with_capacity(file, STREAM_CHUNK_SIZE)) as BlobStream)
            }
        }
    }

    async fn create_dir(&self, parent: &NodeId, name: &str) -> Result<MountStat, DomainError> {
        self.ensure_writable()?;
        validate_name(name)?;
        let parent_path = self.resolve_parent(parent).await?;
        let target = parent_path.join(name);
        tokio::fs::create_dir(&target)
            .await
            .map_err(|e| map_io_err(e, name))?;
        let node_id = Self::child_node_id(parent.as_str(), name);
        self.stat(&node_id).await
    }

    async fn write_stream(
        &self,
        parent: &NodeId,
        name: &str,
        mut body: MountByteStream<'_>,
    ) -> Result<MountStat, DomainError> {
        self.ensure_writable()?;
        validate_name(name)?;
        let parent_path = self.resolve_parent(parent).await?;
        let target = parent_path.join(name);

        // Stream to disk; on ANY error (create / stream / write / flush) the
        // partial file is removed so a retry starts from a clean slate.
        let write_result = async {
            let mut file = tokio::fs::File::create(&target)
                .await
                .map_err(|e| map_io_err(e, name))?;
            while let Some(chunk) = body.next().await {
                let bytes: Bytes = chunk.map_err(|e| {
                    DomainError::internal_error(
                        "ExternalMount",
                        format!("upload stream error: {e}"),
                    )
                })?;
                file.write_all(&bytes)
                    .await
                    .map_err(|e| map_io_err(e, name))?;
            }
            file.flush().await.map_err(|e| map_io_err(e, name))?;
            Ok::<(), DomainError>(())
        }
        .await;

        if let Err(e) = write_result {
            let _ = tokio::fs::remove_file(&target).await;
            return Err(e);
        }

        let node_id = Self::child_node_id(parent.as_str(), name);
        self.stat(&node_id).await
    }

    async fn rename(&self, node_id: &NodeId, new_name: &str) -> Result<MountStat, DomainError> {
        self.ensure_writable()?;
        Self::ensure_not_root(node_id)?;
        validate_name(new_name)?;
        let from = self.resolve_existing(node_id).await?;
        let parent = from.parent().ok_or_else(|| {
            DomainError::operation_not_supported("ExternalMount", "cannot rename mount root")
        })?;
        let to = parent.join(new_name);
        tokio::fs::rename(&from, &to)
            .await
            .map_err(|e| map_io_err(e, new_name))?;
        // Recompute the node id for the new name (same parent).
        let parent_rel = parent_relpath(node_id.as_str());
        let new_node = Self::child_node_id(parent_rel, new_name);
        self.stat(&new_node).await
    }

    async fn delete(&self, node_id: &NodeId) -> Result<(), DomainError> {
        self.ensure_writable()?;
        Self::ensure_not_root(node_id)?;
        let path = self.resolve_existing(node_id).await?;
        let meta = tokio::fs::metadata(&path)
            .await
            .map_err(|e| map_io_err(e, node_id.as_str()))?;
        if meta.is_dir() {
            tokio::fs::remove_dir_all(&path)
                .await
                .map_err(|e| map_io_err(e, node_id.as_str()))
        } else {
            tokio::fs::remove_file(&path)
                .await
                .map_err(|e| map_io_err(e, node_id.as_str()))
        }
    }

    async fn move_within(
        &self,
        node_id: &NodeId,
        dest_parent: &NodeId,
    ) -> Result<MountStat, DomainError> {
        self.ensure_writable()?;
        Self::ensure_not_root(node_id)?;
        let from = self.resolve_existing(node_id).await?;
        let name = node_name(node_id.as_str()).to_string();
        let dest_dir = self.resolve_parent(dest_parent).await?;
        let to = dest_dir.join(&name);
        tokio::fs::rename(&from, &to)
            .await
            .map_err(|e| map_io_err(e, node_id.as_str()))?;
        let new_node = Self::child_node_id(dest_parent.as_str(), &name);
        self.stat(&new_node).await
    }
}

/// Validate a single new name component (mkdir / upload / rename target).
fn validate_name(name: &str) -> Result<(), DomainError> {
    validate_storage_name(name)
        .map_err(|reason| DomainError::validation_error(format!("invalid name '{name}': {reason}")))
}

/// Parent relpath of a node id (`""` when the node is a direct child of root).
fn parent_relpath(relpath: &str) -> &str {
    match relpath.rsplit_once('/') {
        Some((parent, _)) => parent,
        None => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn provider(dir: &Path) -> LocalFsMountProvider {
        LocalFsMountProvider::new(dir, false).expect("provider")
    }

    #[tokio::test]
    async fn lists_and_stats_real_files() {
        let dir = tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("a.txt"), b"hello").unwrap();
        let p = provider(dir.path());

        let mut entries = p.list_dir(&NodeId("".into())).await.unwrap();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "a.txt");
        assert!(!entries[0].is_dir);
        assert_eq!(entries[0].size, 5);
        assert_eq!(entries[1].name, "sub");
        assert!(entries[1].is_dir);

        let stat = p.stat(&NodeId("a.txt".into())).await.unwrap();
        assert_eq!(stat.size, 5);
        assert!(!stat.is_dir);
    }

    #[tokio::test]
    async fn rejects_traversal_and_absolute() {
        let dir = tempdir().unwrap();
        let p = provider(dir.path());
        assert!(p.stat(&NodeId("../escape".into())).await.is_err());
        assert!(p.stat(&NodeId("/etc/passwd".into())).await.is_err());
        assert!(p.stat(&NodeId("a/../../b".into())).await.is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rejects_symlink_escape() {
        let dir = tempdir().unwrap();
        let outside = tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), b"top secret").unwrap();
        std::os::unix::fs::symlink(outside.path().join("secret.txt"), dir.path().join("link"))
            .unwrap();
        let p = provider(dir.path());
        // Stat through the escaping symlink must be rejected...
        assert!(p.stat(&NodeId("link".into())).await.is_err());
        // ...and it must not appear in listings.
        let entries = p.list_dir(&NodeId("".into())).await.unwrap();
        assert!(entries.iter().all(|e| e.name != "link"));
    }

    #[tokio::test]
    async fn read_stream_round_trips_and_ranges() {
        use futures::TryStreamExt;
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("f.bin"), b"0123456789").unwrap();
        let p = provider(dir.path());

        let full: Vec<u8> = p
            .open_read_stream(&NodeId("f.bin".into()), None)
            .await
            .unwrap()
            .map_ok(|b| b.to_vec())
            .try_concat()
            .await
            .unwrap();
        assert_eq!(full, b"0123456789");

        // bytes 2..=5 inclusive => "2345"
        let part: Vec<u8> = p
            .open_read_stream(&NodeId("f.bin".into()), Some((2, Some(5))))
            .await
            .unwrap()
            .map_ok(|b| b.to_vec())
            .try_concat()
            .await
            .unwrap();
        assert_eq!(part, b"2345");
    }

    #[tokio::test]
    async fn write_mkdir_rename_delete_round_trip() {
        use futures::stream;
        let dir = tempdir().unwrap();
        let p = provider(dir.path());

        // mkdir
        let d = p.create_dir(&NodeId("".into()), "folder").await.unwrap();
        assert!(d.is_dir);

        // write into it
        let body: MountByteStream<'static> =
            Box::pin(stream::once(async { Ok(Bytes::from_static(b"data")) }));
        let f = p
            .write_stream(&NodeId("folder".into()), "x.txt", body)
            .await
            .unwrap();
        assert_eq!(f.size, 4);
        assert_eq!(f.node_id.as_str(), "folder/x.txt");

        // rename
        let r = p
            .rename(&NodeId("folder/x.txt".into()), "y.txt")
            .await
            .unwrap();
        assert_eq!(r.node_id.as_str(), "folder/y.txt");

        // delete
        p.delete(&NodeId("folder/y.txt".into())).await.unwrap();
        assert!(p.stat(&NodeId("folder/y.txt".into())).await.is_err());
    }

    #[tokio::test]
    async fn read_only_refuses_mutations() {
        let dir = tempdir().unwrap();
        let p = LocalFsMountProvider::new(dir.path(), true).unwrap();
        assert!(p.create_dir(&NodeId("".into()), "nope").await.is_err());
        assert!(p.capabilities().read_only);
        // Read paths still work on a read-only mount.
        std::fs::write(dir.path().join("a.txt"), b"x").unwrap();
        assert!(p.stat(&NodeId("a.txt".into())).await.is_ok());
    }

    // ── construction ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn new_rejects_missing_path() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("does-not-exist");
        assert!(LocalFsMountProvider::new(&missing, false).is_err());
    }

    #[tokio::test]
    async fn new_rejects_file_as_root() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("f");
        std::fs::write(&file, b"x").unwrap();
        assert!(LocalFsMountProvider::new(&file, false).is_err());
    }

    #[test]
    fn kind_and_capabilities() {
        let dir = tempdir().unwrap();
        let p = provider(dir.path());
        assert_eq!(p.kind(), "local_fs");
        let caps = p.capabilities();
        assert!(caps.supports_range);
        assert!(!caps.read_only);
        assert!(!caps.stable_ids);
    }

    // ── nested listing + node ids ───────────────────────────────────────────

    #[tokio::test]
    async fn lists_nested_directory_with_relative_node_ids() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("a/b")).unwrap();
        std::fs::write(dir.path().join("a/b/c.txt"), b"hi").unwrap();
        let p = provider(dir.path());

        let entries = p.list_dir(&NodeId("a/b".into())).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "c.txt");
        // node id is the FULL relative path, not just the name.
        assert_eq!(entries[0].node_id.as_str(), "a/b/c.txt");

        // resolve_path is identity for local_fs.
        assert_eq!(p.resolve_path("a/b/c.txt").as_str(), "a/b/c.txt");
    }

    #[tokio::test]
    async fn list_dir_skips_unsafe_entry_names() {
        // A file literally named ".." can't exist, but a name with a leading
        // dot is fine; verify dotfiles ARE listed (only traversal tokens are
        // rejected, and the fs never yields "."/"..").
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join(".hidden"), b"x").unwrap();
        let p = provider(dir.path());
        let entries = p.list_dir(&NodeId("".into())).await.unwrap();
        assert!(entries.iter().any(|e| e.name == ".hidden"));
    }

    // ── stat error paths ────────────────────────────────────────────────────

    #[tokio::test]
    async fn stat_directory_reports_dir_and_zero_size() {
        let dir = tempdir().unwrap();
        std::fs::create_dir(dir.path().join("d")).unwrap();
        let p = provider(dir.path());
        let s = p.stat(&NodeId("d".into())).await.unwrap();
        assert!(s.is_dir);
        assert_eq!(s.size, 0);
        assert_eq!(s.mime_type, "directory");
        assert_eq!(s.node_id.as_str(), "d");
    }

    #[tokio::test]
    async fn stat_missing_is_not_found() {
        let dir = tempdir().unwrap();
        let p = provider(dir.path());
        let err = p.stat(&NodeId("nope.txt".into())).await.unwrap_err();
        assert_eq!(err.kind, crate::domain::errors::ErrorKind::NotFound);
    }

    #[tokio::test]
    async fn stat_sniffs_mime_from_extension() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("p.json"), b"{}").unwrap();
        let p = provider(dir.path());
        let s = p.stat(&NodeId("p.json".into())).await.unwrap();
        assert_eq!(s.mime_type, "application/json");
    }

    // ── range edge cases ────────────────────────────────────────────────────

    #[tokio::test]
    async fn range_to_end_when_end_is_none() {
        use futures::TryStreamExt;
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("f"), b"0123456789").unwrap();
        let p = provider(dir.path());
        let out: Vec<u8> = p
            .open_read_stream(&NodeId("f".into()), Some((7, None)))
            .await
            .unwrap()
            .map_ok(|b| b.to_vec())
            .try_concat()
            .await
            .unwrap();
        assert_eq!(out, b"789");
    }

    #[tokio::test]
    async fn range_end_past_eof_is_clamped_by_fs() {
        use futures::TryStreamExt;
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("f"), b"abc").unwrap();
        let p = provider(dir.path());
        // end well past EOF — take() simply yields what exists.
        let out: Vec<u8> = p
            .open_read_stream(&NodeId("f".into()), Some((1, Some(99))))
            .await
            .unwrap()
            .map_ok(|b| b.to_vec())
            .try_concat()
            .await
            .unwrap();
        assert_eq!(out, b"bc");
    }

    #[tokio::test]
    async fn open_missing_file_is_not_found() {
        let dir = tempdir().unwrap();
        let p = provider(dir.path());
        // BlobStream is not Debug, so match rather than unwrap_err.
        let err = match p.open_read_stream(&NodeId("ghost".into()), None).await {
            Ok(_) => panic!("expected an error opening a missing file"),
            Err(e) => e,
        };
        assert_eq!(err.kind, crate::domain::errors::ErrorKind::NotFound);
    }

    // ── create / write error paths ──────────────────────────────────────────

    #[tokio::test]
    async fn create_dir_existing_is_already_exists() {
        let dir = tempdir().unwrap();
        std::fs::create_dir(dir.path().join("d")).unwrap();
        let p = provider(dir.path());
        let err = p.create_dir(&NodeId("".into()), "d").await.unwrap_err();
        assert_eq!(err.kind, crate::domain::errors::ErrorKind::AlreadyExists);
    }

    #[tokio::test]
    async fn create_dir_rejects_invalid_name() {
        let dir = tempdir().unwrap();
        let p = provider(dir.path());
        // names with separators / traversal are validation errors.
        assert!(p.create_dir(&NodeId("".into()), "a/b").await.is_err());
        assert!(p.create_dir(&NodeId("".into()), "..").await.is_err());
        assert!(p.create_dir(&NodeId("".into()), "").await.is_err());
    }

    #[tokio::test]
    async fn create_under_missing_parent_is_not_found() {
        let dir = tempdir().unwrap();
        let p = provider(dir.path());
        let err = p
            .create_dir(&NodeId("ghost".into()), "child")
            .await
            .unwrap_err();
        assert_eq!(err.kind, crate::domain::errors::ErrorKind::NotFound);
    }

    #[tokio::test]
    async fn write_overwrites_existing_file() {
        use futures::stream;
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("f.txt"), b"old-and-longer").unwrap();
        let p = provider(dir.path());
        let body: MountByteStream<'static> =
            Box::pin(stream::once(async { Ok(Bytes::from_static(b"new")) }));
        let s = p
            .write_stream(&NodeId("".into()), "f.txt", body)
            .await
            .unwrap();
        assert_eq!(s.size, 3);
        assert_eq!(
            std::fs::read(dir.path().join("f.txt")).unwrap(),
            b"new".to_vec()
        );
    }

    #[tokio::test]
    async fn write_multi_chunk_concatenates() {
        use futures::stream;
        let dir = tempdir().unwrap();
        let p = provider(dir.path());
        let body: MountByteStream<'static> = Box::pin(stream::iter(vec![
            Ok(Bytes::from_static(b"foo")),
            Ok(Bytes::from_static(b"bar")),
            Ok(Bytes::from_static(b"baz")),
        ]));
        let s = p
            .write_stream(&NodeId("".into()), "m.txt", body)
            .await
            .unwrap();
        assert_eq!(s.size, 9);
        assert_eq!(
            std::fs::read(dir.path().join("m.txt")).unwrap(),
            b"foobarbaz"
        );
    }

    #[tokio::test]
    async fn write_mid_stream_error_removes_partial_file() {
        use futures::stream;
        let dir = tempdir().unwrap();
        let p = provider(dir.path());
        let body: MountByteStream<'static> = Box::pin(stream::iter(vec![
            Ok(Bytes::from_static(b"partial")),
            Err(std::io::Error::other("boom")),
        ]));
        let err = p
            .write_stream(&NodeId("".into()), "broken.txt", body)
            .await
            .unwrap_err();
        assert_eq!(err.kind, crate::domain::errors::ErrorKind::InternalError);
        // The partial file must not be left behind.
        assert!(!dir.path().join("broken.txt").exists());
    }

    // ── root-mutation guard (security) ──────────────────────────────────────

    #[tokio::test]
    async fn root_cannot_be_deleted() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("keep.txt"), b"x").unwrap();
        let p = provider(dir.path());
        let err = p.delete(&NodeId("".into())).await.unwrap_err();
        assert_eq!(
            err.kind,
            crate::domain::errors::ErrorKind::UnsupportedOperation
        );
        // The mount root and its contents survive.
        assert!(dir.path().exists());
        assert!(dir.path().join("keep.txt").exists());
    }

    #[tokio::test]
    async fn root_cannot_be_renamed_or_moved() {
        let dir = tempdir().unwrap();
        std::fs::create_dir(dir.path().join("d")).unwrap();
        let p = provider(dir.path());
        assert!(p.rename(&NodeId("".into()), "escaped").await.is_err());
        assert!(
            p.move_within(&NodeId("".into()), &NodeId("d".into()))
                .await
                .is_err()
        );
        assert!(dir.path().exists());
    }

    // ── rename / move semantics ─────────────────────────────────────────────

    #[tokio::test]
    async fn rename_rejects_invalid_new_name() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), b"x").unwrap();
        let p = provider(dir.path());
        assert!(p.rename(&NodeId("a.txt".into()), "b/c").await.is_err());
        assert!(p.rename(&NodeId("a.txt".into()), "..").await.is_err());
    }

    #[tokio::test]
    async fn move_within_relocates_into_subdir() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), b"x").unwrap();
        std::fs::create_dir(dir.path().join("dest")).unwrap();
        let p = provider(dir.path());
        let s = p
            .move_within(&NodeId("a.txt".into()), &NodeId("dest".into()))
            .await
            .unwrap();
        assert_eq!(s.node_id.as_str(), "dest/a.txt");
        assert!(!dir.path().join("a.txt").exists());
        assert!(dir.path().join("dest/a.txt").exists());
    }

    #[tokio::test]
    async fn move_into_missing_dest_is_not_found() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), b"x").unwrap();
        let p = provider(dir.path());
        let err = p
            .move_within(&NodeId("a.txt".into()), &NodeId("ghost".into()))
            .await
            .unwrap_err();
        assert_eq!(err.kind, crate::domain::errors::ErrorKind::NotFound);
    }

    #[tokio::test]
    async fn delete_directory_recursively() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("d/e")).unwrap();
        std::fs::write(dir.path().join("d/e/f.txt"), b"x").unwrap();
        let p = provider(dir.path());
        p.delete(&NodeId("d".into())).await.unwrap();
        assert!(!dir.path().join("d").exists());
    }

    // ── map_io_err mapping table ────────────────────────────────────────────

    #[test]
    fn io_error_mapping() {
        use crate::domain::errors::ErrorKind;
        use std::io::{Error, ErrorKind as Io};
        assert_eq!(
            map_io_err(Error::from(Io::NotFound), "x").kind,
            ErrorKind::NotFound
        );
        assert_eq!(
            map_io_err(Error::from(Io::PermissionDenied), "x").kind,
            ErrorKind::AccessDenied
        );
        assert_eq!(
            map_io_err(Error::from(Io::AlreadyExists), "x").kind,
            ErrorKind::AlreadyExists
        );
        assert_eq!(
            map_io_err(Error::other("weird"), "x").kind,
            ErrorKind::InternalError
        );
    }

    #[test]
    fn parent_relpath_and_node_name_helpers() {
        assert_eq!(parent_relpath("a/b/c"), "a/b");
        assert_eq!(parent_relpath("top"), "");
        assert_eq!(node_name("a/b/c.txt"), "c.txt");
        assert_eq!(node_name("solo"), "solo");
    }
    #[tokio::test]
    async fn lists_empty_directory() {
        let dir = tempdir().unwrap();
        std::fs::create_dir(dir.path().join("empty")).unwrap();
        let p = provider(dir.path());
        let entries = p.list_dir(&NodeId("empty".into())).await.unwrap();
        assert!(entries.is_empty());
    }

    #[tokio::test]
    async fn list_dir_on_a_file_errors() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("f.txt"), b"x").unwrap();
        let p = provider(dir.path());
        assert!(p.list_dir(&NodeId("f.txt".into())).await.is_err());
    }

    #[tokio::test]
    async fn stat_root_reports_directory() {
        let dir = tempdir().unwrap();
        let p = provider(dir.path());
        let s = p.stat(&NodeId("".into())).await.unwrap();
        assert!(s.is_dir);
        assert_eq!(s.node_id.as_str(), "");
    }

    #[tokio::test]
    async fn write_empty_file_zero_chunks() {
        use futures::stream;
        let dir = tempdir().unwrap();
        let p = provider(dir.path());
        let body: MountByteStream<'static> = Box::pin(stream::empty());
        let s = p
            .write_stream(&NodeId("".into()), "empty.txt", body)
            .await
            .unwrap();
        assert_eq!(s.size, 0);
        assert!(dir.path().join("empty.txt").exists());
    }

    #[tokio::test]
    async fn rename_onto_existing_sibling() {
        // POSIX rename replaces an existing file at the destination.
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), b"aaa").unwrap();
        std::fs::write(dir.path().join("b.txt"), b"b").unwrap();
        let p = provider(dir.path());
        let r = p.rename(&NodeId("a.txt".into()), "b.txt").await.unwrap();
        assert_eq!(r.node_id.as_str(), "b.txt");
        assert_eq!(r.size, 3);
        assert!(!dir.path().join("a.txt").exists());
    }

    #[tokio::test]
    async fn move_dir_into_own_subtree_is_rejected_by_os() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("d/inner")).unwrap();
        let p = provider(dir.path());
        // Moving "d" into "d/inner" is a cycle; the OS refuses it.
        assert!(
            p.move_within(&NodeId("d".into()), &NodeId("d/inner".into()))
                .await
                .is_err()
        );
        assert!(dir.path().join("d/inner").exists());
    }

    #[tokio::test]
    async fn unicode_filenames_round_trip() {
        use futures::TryStreamExt;
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("café.txt"), b"\xc3\xa9").unwrap();
        let p = provider(dir.path());
        let entries = p.list_dir(&NodeId("".into())).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "café.txt");
        assert_eq!(entries[0].node_id.as_str(), "café.txt");
        let bytes: Vec<u8> = p
            .open_read_stream(&entries[0].node_id, None)
            .await
            .unwrap()
            .map_ok(|b| b.to_vec())
            .try_concat()
            .await
            .unwrap();
        assert_eq!(bytes, b"\xc3\xa9");
    }

    #[tokio::test]
    async fn read_only_refuses_all_mutators() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), b"x").unwrap();
        std::fs::create_dir(dir.path().join("dest")).unwrap();
        let p = LocalFsMountProvider::new(dir.path(), true).unwrap();
        use futures::stream;
        let body: MountByteStream<'static> =
            Box::pin(stream::once(async { Ok(Bytes::from_static(b"x")) }));
        assert!(
            p.write_stream(&NodeId("".into()), "n.txt", body)
                .await
                .is_err()
        );
        assert!(p.rename(&NodeId("a.txt".into()), "b.txt").await.is_err());
        assert!(p.delete(&NodeId("a.txt".into())).await.is_err());
        assert!(
            p.move_within(&NodeId("a.txt".into()), &NodeId("dest".into()))
                .await
                .is_err()
        );
        // Read paths still work.
        assert!(p.stat(&NodeId("a.txt".into())).await.is_ok());
    }
}
