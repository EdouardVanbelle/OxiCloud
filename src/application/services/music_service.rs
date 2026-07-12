use std::collections::HashSet;
use std::sync::Arc;
use uuid::Uuid;

use crate::application::dtos::playlist_dto::{
    AddTracksDto, AudioMetadataDto, CreatePlaylistDto, PlaylistDto, PlaylistItemDto,
    PlaylistQueryDto, PlaylistShareInfoDto, ReorderTracksDto, SharePlaylistDto, UpdatePlaylistDto,
};
use crate::application::ports::authorization_ports::AuthorizationEngine;
use crate::application::ports::music_ports::{MusicStoragePort, MusicUseCase};
use crate::common::errors::{DomainError, ErrorKind};
use crate::domain::services::authorization::{Permission, Resource, Role, Subject};
use crate::infrastructure::adapters::music_storage_adapter::MusicStorageAdapter;
use crate::infrastructure::services::pg_acl_engine::PgAclEngine;

/// Music service — the REST entry point for every playlist or audio
/// metadata operation. Every method routes through
/// `AuthorizationEngine`; the pre-Round-3 `user_has_access` /
/// `user_can_write` bespoke helpers on `MusicStorageAdapter` are no
/// longer consulted for access decisions.
///
/// Ownership + sharing live entirely in `storage.role_grants`
/// (`resource_type='playlist'`). `audio.playlists.owner_id` stays for
/// provenance and legacy queries; `audio.playlist_shares` is
/// backfilled and slated for removal in a follow-up migration.
pub struct MusicService {
    storage: Arc<MusicStorageAdapter>,
    /// ReBAC engine — every user-facing method calls `authz.require`
    /// with the appropriate `Permission`. `create_playlist` also uses
    /// it to seed an Owner grant for the caller, so the common
    /// "owning my own playlist" case takes a single indexed
    /// role_grants lookup on subsequent reads.
    authz: Arc<PgAclEngine>,
}

impl MusicService {
    pub fn new(storage: Arc<MusicStorageAdapter>, authz: Arc<PgAclEngine>) -> Self {
        Self { storage, authz }
    }

    /// Parse `playlist_id` and enforce `permission` on
    /// `Resource::Playlist(uuid)`. On denial `authz.require` returns
    /// `NotFound` (anti-enum — same shape as "no such playlist") and
    /// emits the `authz.denied` audit line. Returns the parsed UUID
    /// on success so the caller doesn't have to parse it a second
    /// time.
    async fn require_playlist_perm(
        &self,
        playlist_id: &str,
        caller_id: Uuid,
        permission: Permission,
    ) -> Result<Uuid, DomainError> {
        let uuid = Uuid::parse_str(playlist_id)
            .map_err(|_| DomainError::new(ErrorKind::InvalidInput, "Playlist", "Invalid ID"))?;
        self.authz
            .require(
                Subject::User(caller_id),
                permission,
                Resource::Playlist(uuid),
            )
            .await?;
        Ok(uuid)
    }

    /// Check `permission` on a playlist without throwing. Used by the
    /// read paths that also allow a public-playlist bypass — they
    /// need a bool, not a `Result<(), NotFound>`.
    async fn has_playlist_perm(
        &self,
        playlist_id: &str,
        caller_id: Uuid,
        permission: Permission,
    ) -> Result<bool, DomainError> {
        let uuid = Uuid::parse_str(playlist_id)
            .map_err(|_| DomainError::new(ErrorKind::InvalidInput, "Playlist", "Invalid ID"))?;
        self.authz
            .check(
                Subject::User(caller_id),
                permission,
                Resource::Playlist(uuid),
            )
            .await
    }
}

impl MusicUseCase for MusicService {
    async fn create_playlist(
        &self,
        dto: CreatePlaylistDto,
        user_id: Uuid,
    ) -> Result<PlaylistDto, DomainError> {
        // No pre-write gate: creating a playlist is a personal act.
        // Storage stamps `owner_id = user_id`; we then seed an Owner
        // role_grant so subsequent reads hit the same
        // `storage.role_grants` fast path used everywhere else.
        let created = self.storage.create_playlist(dto, user_id).await?;
        let playlist_uuid = Uuid::parse_str(&created.id).map_err(|_| {
            DomainError::internal_error("Playlist", "storage returned invalid playlist id")
        })?;
        // `set_role` is idempotent on the `(subject, resource)` unique
        // key. `granted_by = user_id` is the self-seeded creation event.
        self.authz
            .set_role(
                user_id,
                Subject::User(user_id),
                Role::Owner,
                Resource::Playlist(playlist_uuid),
                None,
            )
            .await?;
        Ok(created)
    }

    async fn update_playlist(
        &self,
        playlist_id: &str,
        dto: UpdatePlaylistDto,
        user_id: Uuid,
    ) -> Result<PlaylistDto, DomainError> {
        self.require_playlist_perm(playlist_id, user_id, Permission::Update)
            .await?;
        self.storage.update_playlist(playlist_id, dto).await
    }

    async fn delete_playlist(&self, playlist_id: &str, user_id: Uuid) -> Result<(), DomainError> {
        let uuid = self
            .require_playlist_perm(playlist_id, user_id, Permission::Delete)
            .await?;
        self.storage.delete_playlist(playlist_id).await?;
        // Wipe every grant on this playlist so a re-used UUID
        // (impossible today but cheap to defend against) doesn't
        // inherit stale ACLs. The storage DELETE won't cascade to
        // `storage.role_grants` — it's cross-schema.
        let _ = self
            .authz
            .revoke_all_for_resource(Resource::Playlist(uuid))
            .await;
        Ok(())
    }

    async fn get_playlist(
        &self,
        playlist_id: &str,
        user_id: Uuid,
    ) -> Result<PlaylistDto, DomainError> {
        let playlist = self.storage.get_playlist(playlist_id).await?;
        let playlist = match playlist {
            Some(p) => p,
            None => return Err(DomainError::not_found("Playlist", playlist_id)),
        };
        // Public-playlist bypass: anonymous-ish read. `check` returns
        // bool (no throw); combine with the public flag before
        // deciding.
        let allowed = playlist.is_public
            || self
                .has_playlist_perm(playlist_id, user_id, Permission::Read)
                .await?;
        if !allowed {
            return Err(DomainError::not_found("Playlist", playlist_id));
        }
        Ok(playlist)
    }

    async fn list_playlists(
        &self,
        query: PlaylistQueryDto,
        user_id: Uuid,
    ) -> Result<Vec<PlaylistDto>, DomainError> {
        let include_shared = query.include_shared.unwrap_or(true);
        let include_public = query.include_public.unwrap_or(false);
        let limit = query.limit.unwrap_or(100);
        let offset = query.offset.unwrap_or(0);

        // Post-Round-3 semantics: playlists the caller has any grant
        // on come from `list_incoming_grants` — one union of owned +
        // shared. The pre-Round-3 code fetched them via two separate
        // queries (`list_playlists_by_owner` + `list_shared_with_user`)
        // that each read a different table.
        let grants = self
            .authz
            .list_incoming_grants(Subject::User(user_id))
            .await?;

        // Deduplicate — a user can hold multiple grants on the same
        // playlist (direct + group-inherited). We only need one DTO
        // per resource.
        let mut playlist_ids: HashSet<Uuid> = grants
            .into_iter()
            .filter_map(|g| match g.resource {
                Resource::Playlist(id) => Some(id),
                _ => None,
            })
            .collect();

        // `include_shared=false` narrows the listing to owned playlists
        // only. Owner is a grant like any other in `role_grants`, so we
        // filter the aggregated set against the owner_id stamped on
        // each row after hydration — cheaper than a second SQL round-trip.
        let mut playlists: Vec<PlaylistDto> = Vec::with_capacity(playlist_ids.len());
        let user_str = user_id.to_string();
        for id in playlist_ids.drain() {
            if let Ok(Some(p)) = self.storage.get_playlist(&id.to_string()).await
                && (include_shared || p.owner_id == user_str)
            {
                playlists.push(p);
            }
        }

        if include_public {
            let public = self.storage.list_public_playlists(limit, offset).await?;
            for p in public {
                if !playlists.iter().any(|pl: &PlaylistDto| pl.id == p.id) {
                    playlists.push(p);
                }
            }
        }

        Ok(playlists)
    }

    async fn add_tracks(
        &self,
        playlist_id: &str,
        dto: AddTracksDto,
        user_id: Uuid,
    ) -> Result<Vec<PlaylistItemDto>, DomainError> {
        let playlist_uuid = self
            .require_playlist_perm(playlist_id, user_id, Permission::Update)
            .await?;

        let file_ids: Result<Vec<Uuid>, _> =
            dto.file_ids.iter().map(|id| Uuid::parse_str(id)).collect();
        let file_ids = file_ids.map_err(|_| {
            DomainError::new(ErrorKind::InvalidInput, "Playlist", "Invalid file ID")
        })?;

        self.storage.add_tracks(&playlist_uuid, &file_ids).await
    }

    async fn remove_track(
        &self,
        playlist_id: &str,
        file_id: &str,
        user_id: Uuid,
    ) -> Result<(), DomainError> {
        let playlist_uuid = self
            .require_playlist_perm(playlist_id, user_id, Permission::Update)
            .await?;
        let file_uuid = Uuid::parse_str(file_id).map_err(|_| {
            DomainError::new(ErrorKind::InvalidInput, "Playlist", "Invalid file ID")
        })?;
        self.storage.remove_track(&playlist_uuid, &file_uuid).await
    }

    async fn reorder_tracks(
        &self,
        playlist_id: &str,
        dto: ReorderTracksDto,
        user_id: Uuid,
    ) -> Result<(), DomainError> {
        let playlist_uuid = self
            .require_playlist_perm(playlist_id, user_id, Permission::Update)
            .await?;

        let item_ids: Result<Vec<Uuid>, _> =
            dto.item_ids.iter().map(|id| Uuid::parse_str(id)).collect();
        let item_ids = item_ids.map_err(|_| {
            DomainError::new(ErrorKind::InvalidInput, "Playlist", "Invalid item ID")
        })?;

        self.storage.reorder_tracks(&playlist_uuid, &item_ids).await
    }

    async fn list_playlist_tracks(
        &self,
        playlist_id: &str,
        user_id: Uuid,
    ) -> Result<Vec<PlaylistItemDto>, DomainError> {
        let playlist_uuid = Uuid::parse_str(playlist_id).map_err(|_| {
            DomainError::new(ErrorKind::InvalidInput, "Playlist", "Invalid playlist ID")
        })?;
        // Public-playlist bypass mirrors `get_playlist`: readers of a
        // public playlist can see its tracks. Fetch the playlist row
        // to inspect `is_public` before deciding.
        let playlist = self
            .storage
            .get_playlist(playlist_id)
            .await?
            .ok_or_else(|| DomainError::not_found("Playlist", playlist_id))?;
        let allowed = playlist.is_public
            || self
                .has_playlist_perm(playlist_id, user_id, Permission::Read)
                .await?;
        if !allowed {
            return Err(DomainError::not_found("Playlist", playlist_id));
        }
        self.storage.list_playlist_tracks(&playlist_uuid).await
    }

    async fn share_playlist(
        &self,
        playlist_id: &str,
        dto: SharePlaylistDto,
        caller_id: Uuid,
    ) -> Result<(), DomainError> {
        let playlist_uuid = self
            .require_playlist_perm(playlist_id, caller_id, Permission::Share)
            .await?;
        let target_user_id = Uuid::parse_str(&dto.user_id).map_err(|_| {
            DomainError::new(ErrorKind::InvalidInput, "Playlist", "Invalid user ID")
        })?;
        // Legacy `can_write` boolean maps into the role bundle system:
        //   - false → Viewer (Read only)
        //   - true  → Editor (Read + Update)
        // The endpoint stays boolean-shaped for API back-compat; new
        // integrations should switch to the unified `/api/grants` API
        // which exposes the full role set.
        let role = if dto.can_write.unwrap_or(false) {
            Role::Editor
        } else {
            Role::Viewer
        };
        self.authz
            .set_role(
                caller_id,
                Subject::User(target_user_id),
                role,
                Resource::Playlist(playlist_uuid),
                None,
            )
            .await?;
        Ok(())
    }

    async fn remove_share(
        &self,
        playlist_id: &str,
        target_user_id: &str,
        caller_id: Uuid,
    ) -> Result<(), DomainError> {
        let playlist_uuid = self
            .require_playlist_perm(playlist_id, caller_id, Permission::Share)
            .await?;
        let target_uuid = Uuid::parse_str(target_user_id).map_err(|_| {
            DomainError::new(ErrorKind::InvalidInput, "Playlist", "Invalid user ID")
        })?;
        self.authz
            .clear_role(
                Subject::User(target_uuid),
                Resource::Playlist(playlist_uuid),
            )
            .await
    }

    async fn get_playlist_shares(
        &self,
        playlist_id: &str,
        user_id: Uuid,
    ) -> Result<Vec<PlaylistShareInfoDto>, DomainError> {
        let playlist_uuid = self
            .require_playlist_perm(playlist_id, user_id, Permission::Share)
            .await?;
        // `list_grants_on_resource` returns every role_grant row for
        // the playlist. Drop the Owner self-grant seeded at creation
        // (the caller already knows they own it) and collapse the
        // role bundle back to a boolean `can_write` for the legacy
        // DTO shape.
        let grants = self
            .authz
            .list_grants_on_resource(Resource::Playlist(playlist_uuid))
            .await?;
        Ok(grants
            .into_iter()
            .filter_map(|g| match g.subject {
                Subject::User(uid) if g.role != Role::Owner => Some(PlaylistShareInfoDto {
                    user_id: uid.to_string(),
                    can_write: g.role.expand().contains(&Permission::Update),
                }),
                _ => None,
            })
            .collect())
    }

    async fn get_audio_metadata(
        &self,
        file_id: &str,
        caller_id: Uuid,
    ) -> Result<Option<AudioMetadataDto>, DomainError> {
        let file_uuid = Uuid::parse_str(file_id)
            .map_err(|_| DomainError::new(ErrorKind::InvalidInput, "Music", "Invalid file ID"))?;
        // AuthZ pre-read: caller must have `Read` on the underlying
        // audio file. Before this check the endpoint returned
        // metadata for any known file id (cross-tenant IDOR — the
        // `_user_id` parameter was deliberately unused). `require`
        // returns 404 on denial to match the anti-enum shape used
        // everywhere else.
        self.authz
            .require(
                Subject::User(caller_id),
                Permission::Read,
                Resource::File(file_uuid),
            )
            .await?;
        self.storage.get_audio_metadata(&file_uuid).await
    }
}
