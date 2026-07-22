//! Streams an upload straight to an external mount's provider, bypassing the
//! content-addressable store entirely (no BLAKE3 / dedup).
//!
//! The REST upload handler detects a mount destination BEFORE ingesting into the
//! CAS and routes here. Authorization stays in this service (the mount-root
//! `Create` grant); the handler only classifies and supplies the body stream.

use std::sync::Arc;

use crate::application::dtos::file_dto::FileDto;
use crate::application::ports::authorization_ports::AuthorizationEngine;
use crate::application::ports::external_mount_ports::MountByteStream;
use crate::application::services::mount_dto::{audit_mount_write, mount_file_dto, mount_parent_id};
use crate::application::services::mount_registry::MountConfig;
use crate::common::errors::DomainError;
use crate::domain::services::authorization::{Permission, Resource, Subject};
use crate::domain::services::external_mount_id::NodeId;
use crate::infrastructure::services::pg_acl_engine::PgAclEngine;
use uuid::Uuid;

/// Writes uploaded bytes to a mount provider with authorization + auditing.
pub struct ExternalUploadService {
    authz: Arc<PgAclEngine>,
}

impl ExternalUploadService {
    /// Construct over the ReBAC engine.
    pub fn new(authz: Arc<PgAclEngine>) -> Self {
        Self { authz }
    }

    /// Authorize (`Create` on the mount root) then stream `body` to the provider
    /// as `name` under `parent_node`. Returns the synthesized `FileDto`.
    pub async fn write_file(
        &self,
        cfg: &MountConfig,
        parent_node: &NodeId,
        name: &str,
        body: MountByteStream<'_>,
        caller_id: Uuid,
    ) -> Result<FileDto, DomainError> {
        self.authz
            .require(
                Subject::User(caller_id),
                Permission::Create,
                Resource::Folder(cfg.mount_id),
            )
            .await?;

        let stat = cfg.provider.write_stream(parent_node, name, body).await?;
        audit_mount_write("upload", cfg, caller_id, stat.node_id.as_str());
        let parent = mount_parent_id(cfg, stat.node_id.as_str());
        Ok(mount_file_dto(cfg, &parent, &stat))
    }
}
