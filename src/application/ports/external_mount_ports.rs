//! External mount provider port — the pluggable backend abstraction for mounts.
//!
//! An [`ExternalMountProvider`] exposes a filesystem-style I/O surface for one
//! mount's backend (raw host fs in v1; SFTP/WebDAV/… later). It is the *lowest
//! common denominator* of browse + CRUD: deliberately small, so new backend
//! `kind`s drop in without touching the router, listing, authz, or path
//! resolution. Rich native features (sharing, trash, search, …) are NOT part of
//! this trait — they compose above it.
//!
//! Each provider instance is **bound to one mount's root location at
//! construction**, so methods take only a provider-owned [`NodeId`], never a host
//! `Path` (an SFTP/WebDAV provider has no local path). The `NodeId` is opaque to
//! the rest of the system (see [`crate::domain::services::external_mount_id`]).
//!
//! The trait returns boxed futures via `#[async_trait]` and takes a boxed write
//! stream, so it is dyn-compatible (`Arc<dyn ExternalMountProvider>`) and a
//! single mount registry can hold providers of different kinds.

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;
use std::pin::Pin;
use uuid::Uuid;

use crate::application::ports::blob_storage_ports::BlobStream;
use crate::domain::errors::DomainError;
use crate::domain::services::external_mount_id::NodeId;

/// A byte stream handed to [`ExternalMountProvider::write_stream`].
///
/// Boxed (not generic) so the trait stays object-safe. Callers map their body's
/// error type to `std::io::Error` before constructing it.
pub type MountByteStream<'a> =
    Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send + 'a>>;

/// One entry returned by [`ExternalMountProvider::list_dir`].
#[derive(Debug, Clone)]
pub struct MountEntry {
    /// Final path segment (display name).
    pub name: String,
    /// Provider-assigned, opaque identity for this entry.
    pub node_id: NodeId,
    /// Whether the entry is a directory.
    pub is_dir: bool,
    /// Size in bytes (0 for directories).
    pub size: u64,
    /// Last-modified time, unix seconds.
    pub modified_at: u64,
    /// Creation time, unix seconds (falls back to `modified_at` when unavailable).
    pub created_at: u64,
}

/// Metadata for a single entry returned by [`ExternalMountProvider::stat`] and
/// by the mutating ops (so the caller learns the new entry's `node_id`).
#[derive(Debug, Clone)]
pub struct MountStat {
    /// Provider-assigned, opaque identity for this entry.
    pub node_id: NodeId,
    /// Whether the entry is a directory.
    pub is_dir: bool,
    /// Size in bytes (0 for directories).
    pub size: u64,
    /// Last-modified time, unix seconds.
    pub modified_at: u64,
    /// Creation time, unix seconds.
    pub created_at: u64,
    /// MIME type (sniffed from extension for files; `"directory"` for dirs).
    pub mime_type: String,
}

/// Static capability flags a provider advertises.
#[derive(Debug, Clone, Copy)]
pub struct MountCaps {
    /// Provider can serve byte ranges (HTTP Range / partial reads).
    pub supports_range: bool,
    /// Provider refuses all mutations.
    pub read_only: bool,
    /// `node_id`s are stable across renames/moves (e.g. inode / object id).
    /// `false` for path-based providers — relevant to the future sharing path.
    pub stable_ids: bool,
}

/// Pluggable I/O surface for one mount's backend, bound to its root location.
///
/// Implementations: `LocalFsMountProvider` (v1). All node ids are
/// provider-owned and opaque; the system never parses them.
#[async_trait]
pub trait ExternalMountProvider: Send + Sync + 'static {
    /// Provider kind identifier (matches the `kind` column / factory arm).
    fn kind(&self) -> &'static str;

    /// Static capabilities.
    fn capabilities(&self) -> MountCaps;

    /// Map an internal path (relative to the mount root) to a `node_id`.
    ///
    /// For path-based providers this is identity (the default). Providers whose
    /// identity is not a path override this. Does not assert existence — use
    /// [`stat`](Self::stat) for that.
    fn resolve_path(&self, path: &str) -> NodeId {
        NodeId(path.to_string())
    }

    /// List the directory identified by `node_id` (root = the provider's bound
    /// location, addressed via `resolve_path("")`).
    async fn list_dir(&self, node_id: &NodeId) -> Result<Vec<MountEntry>, DomainError>;

    /// Stat a single entry.
    async fn stat(&self, node_id: &NodeId) -> Result<MountStat, DomainError>;

    /// Open a (optionally ranged) read stream over a file's bytes.
    ///
    /// `range` is `(start, end_inclusive_opt)`; `None` reads the whole file.
    async fn open_read_stream(
        &self,
        node_id: &NodeId,
        range: Option<(u64, Option<u64>)>,
    ) -> Result<BlobStream, DomainError>;

    /// Create a child directory `name` under `parent`. Returns the new dir's stat.
    async fn create_dir(&self, parent: &NodeId, name: &str) -> Result<MountStat, DomainError>;

    /// Stream-write a child file `name` under `parent`. Returns the new file's stat.
    async fn write_stream(
        &self,
        parent: &NodeId,
        name: &str,
        body: MountByteStream<'_>,
    ) -> Result<MountStat, DomainError>;

    /// Rename an entry in place (same parent). Returns the renamed entry's stat.
    async fn rename(&self, node_id: &NodeId, new_name: &str) -> Result<MountStat, DomainError>;

    /// Delete an entry (recursively for directories). Permanent — no trash.
    async fn delete(&self, node_id: &NodeId) -> Result<(), DomainError>;

    /// Move an entry into `dest_parent`, keeping its name. Returns the new stat.
    async fn move_within(
        &self,
        node_id: &NodeId,
        dest_parent: &NodeId,
    ) -> Result<MountStat, DomainError>;
}

/// A persisted external mount joined with its mount-root folder.
///
/// Returned by [`ExternalMountRepositoryPort::list_all`] to (re)build the
/// in-memory registry.
#[derive(Debug, Clone)]
pub struct ExternalMountRecord {
    /// The mount-root folder UUID (also the mount's identity in the registry).
    pub mount_folder_id: Uuid,
    /// Provider kind (factory discriminator).
    pub kind: String,
    /// Provider-specific connection config.
    pub config: serde_json::Value,
    /// Display name.
    pub name: String,
    /// Owner of the mount configuration.
    pub owner_id: Uuid,
    /// Whether the mount is read-only.
    pub read_only: bool,
    /// Drive the mount-root folder belongs to (for path resolution).
    pub drive_id: Uuid,
    /// Materialized internal path of the mount-root folder (for path resolution).
    pub mount_path: String,
}

/// The persistable columns of an `external_mounts` row (admin create).
#[derive(Debug, Clone)]
pub struct NewExternalMount {
    /// The mount-root folder UUID this mount attaches to.
    pub mount_folder_id: Uuid,
    /// Provider kind.
    pub kind: String,
    /// Provider-specific connection config.
    pub config: serde_json::Value,
    /// Display name.
    pub name: String,
    /// Owner of the mount configuration.
    pub owner_id: Uuid,
    /// Whether the mount is read-only.
    pub read_only: bool,
}

/// Persistence port for external mount configuration.
#[async_trait]
pub trait ExternalMountRepositoryPort: Send + Sync {
    /// Load every (non-trashed) mount joined with its folder, for registry build.
    async fn list_all(&self) -> Result<Vec<ExternalMountRecord>, DomainError>;

    /// Insert a new mount row. Default errors — only the PG repo implements it
    /// (test doubles need `list_all` only).
    async fn create(&self, _mount: &NewExternalMount) -> Result<(), DomainError> {
        Err(DomainError::operation_not_supported(
            "ExternalMount",
            "create is not supported by this repository",
        ))
    }

    /// Delete a mount row by its mount-root folder id. Returns `true` when a
    /// row was removed. Default errors (see `create`).
    async fn delete(&self, _mount_folder_id: Uuid) -> Result<bool, DomainError> {
        Err(DomainError::operation_not_supported(
            "ExternalMount",
            "delete is not supported by this repository",
        ))
    }
}

/// Builds [`ExternalMountProvider`]s from a `kind` + `config` pair.
///
/// The single extension point for new backends: adding a provider is
/// implementing the trait plus one arm here.
#[async_trait]
pub trait MountProviderFactory: Send + Sync {
    /// Construct a provider for `kind`, parsing its `config` JSON.
    ///
    /// Errors with `UnsupportedOperation` for an unknown kind, or
    /// `validation_error` for malformed config.
    async fn build(
        &self,
        kind: &str,
        config: &serde_json::Value,
    ) -> Result<Arc<dyn ExternalMountProvider>, DomainError>;
}
