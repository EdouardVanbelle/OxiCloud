//! Authorization port — the trait every service depends on for permission
//! decisions. Implementations: `PgAclEngine` (v1 default), `OpenFgaEngine`
//! (future). A `CachedAuthorizationEngine` decorator over either is planned
//! as a future optimization.
//!
//! Architectural rule (see CLAUDE.md):
//! **AuthZ is enforced exclusively in the application service layer.**
//! Handlers authenticate the caller and pass `caller_id` to the service;
//! they never call this trait directly.

use uuid::Uuid;

use crate::common::errors::DomainError;
use crate::domain::services::authorization::{
    Grant, GrantCursor, IncomingGrantSummary, OutgoingResourceSummary, Permission, Resource,
    ResourceKind, Role, Subject,
};

pub trait AuthorizationEngine: Send + Sync + 'static {
    /// Returns true if `subject` has `permission` on `resource`, considering
    /// owner short-circuit AND cascading from folder ancestors.
    ///
    /// `check` never errors for "permission denied" — that's a `false` return.
    /// `Err` is reserved for infrastructure failures (DB down, etc.).
    async fn check(
        &self,
        subject: Subject,
        permission: Permission,
        resource: Resource,
    ) -> Result<bool, DomainError>;

    /// Convenience wrapper around `check`: returns `Ok(())` when allowed and
    /// `DomainError::not_found` when denied (anti-enumeration — same error as
    /// "resource doesn't exist" so attackers can't probe IDs by error shape).
    async fn require(
        &self,
        subject: Subject,
        permission: Permission,
        resource: Resource,
    ) -> Result<(), DomainError> {
        if self.check(subject, permission, resource).await? {
            // Granted path: high-traffic (every authorized request hits
            // this), so kept at `debug` and structured for grep-friendly
            // filtering. Not an audit event — the audit trail focuses
            // on denials and explicit mutations elsewhere.
            tracing::debug!(
                target: "oxicloud::authz",
                event = "authz.allowed",
                subject_type = subject.type_str(),
                subject_id = %subject.id(),
                permission = permission.as_str(),
                resource_type = resource.type_str(),
                resource_id = %resource.id(),
                "👮🏻‍♂️ perms: ✔ Subject '{}' has permission to '{}' on resource '{}'",
                subject,
                permission,
                resource
            );
            Ok(())
        } else {
            let (kind, id) = match resource {
                Resource::Folder(id) => ("Folder", id),
                Resource::File(id) => ("File", id),
                Resource::Drive(id) => ("Drive", id),
                Resource::Calendar(id) => ("Calendar", id),
                Resource::AddressBook(id) => ("AddressBook", id),
                Resource::Playlist(id) => ("Playlist", id),
            };
            // Audit-worthy: denials are the interesting signal. Routed
            // through the `audit` tracing target so log aggregators can
            // surface them separately from operational debug traffic.
            // Span context (request_id, client_ip, user_id) is attached
            // automatically by the request-scope span set in
            // `interfaces/middleware/trace_span.rs`, so this log line
            // doesn't need to duplicate those fields — they appear in
            // the structured output of every log written inside the
            // request span.
            tracing::info!(
                target: "audit",
                event = "authz.denied",
                subject_type = subject.type_str(),
                subject_id = %subject.id(),
                permission = permission.as_str(),
                resource_type = resource.type_str(),
                resource_id = %resource.id(),
                "👮🏻‍♂️ perms: ⛔ Subject '{}' hasn't permission to '{}' on resource '{}'",
                subject,
                permission,
                resource
            );
            Err(DomainError::not_found(kind, id.to_string()))
        }
    }

    /// Resources explicitly granted to `subject`. Direct grants only — no
    /// cascade expansion. Used by `GET /api/grants/incoming`.
    async fn list_incoming_grants(&self, subject: Subject) -> Result<Vec<Grant>, DomainError>;

    /// Cursor-paginated list of resources explicitly granted to `subject`,
    /// optionally filtered by resource kind. Multiple permission rows for the
    /// same resource are collapsed into one `IncomingGrantSummary`.
    ///
    /// Ordered by `MIN(granted_at) DESC, resource_id DESC` — stable across
    /// concurrent inserts because the cursor encodes both fields.
    ///
    /// Pass `kinds = &[]` to return all resource kinds.
    /// Returns `(summaries, next_cursor)` — `next_cursor` is `None` when the
    /// last page has been reached.
    async fn list_incoming_resources_paged(
        &self,
        subject: Subject,
        kinds: &[ResourceKind],
        limit: u32,
        cursor: Option<GrantCursor>,
        sort_by: &str,
        reverse: bool,
    ) -> Result<(Vec<IncomingGrantSummary>, Option<GrantCursor>), DomainError>;

    /// All grants on a specific resource (for "Manage sharing" UI). Caller
    /// must verify the caller has `Share` on the resource before invoking.
    async fn list_grants_on_resource(&self, resource: Resource) -> Result<Vec<Grant>, DomainError>;

    /// Grants Outgoing — grants created by `granted_by`. Used by
    /// `GET /api/grants/outgoing` ("things I've shared with others").
    async fn list_outgoing_grants(&self, granted_by: Uuid) -> Result<Vec<Grant>, DomainError>;

    /// Cursor-paginated list of resources that `granted_by` has shared with
    /// others. Multiple permission rows for the same (subject, resource) pair
    /// are collapsed into one `OutgoingGrantEntry`; multiple subjects on the
    /// same resource are grouped into one `OutgoingResourceSummary`.
    ///
    /// Returns `(summaries, next_cursor)`.
    async fn list_outgoing_resources_paged(
        &self,
        granted_by: Uuid,
        limit: u32,
        cursor: Option<GrantCursor>,
        sort_by: &str,
        reverse: bool,
    ) -> Result<(Vec<OutgoingResourceSummary>, Option<GrantCursor>), DomainError>;

    /// Update `expires_at` for every role grant belonging to `subject`.
    /// Used by `share_service` when a token-share's expiry is refreshed —
    /// the subject (token) maps to a small fixed set of role grants, so a
    /// single UPDATE covers them. Resource-scoped expiry changes go through
    /// `set_role` (which carries `expires_at` as part of its UPSERT).
    async fn set_expiry_for_subject(
        &self,
        subject: Subject,
        expires_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<(), DomainError>;

    /// Revoke a single role grant by its UUID. Idempotent — returns `Ok(())`
    /// whether or not the row existed. The id comes from a prior listing
    /// or `find_grant_full_by_id` lookup.
    async fn revoke(&self, grant_id: Uuid) -> Result<(), DomainError>;

    /// Removes every grant whose `resource` matches. Called by lifecycle
    /// hooks when a resource is permanently deleted. Returns the count of
    /// rows removed.
    async fn revoke_all_for_resource(&self, resource: Resource) -> Result<usize, DomainError>;

    /// Removes every grant whose `subject` matches. Called when a user/token
    /// /group is deleted. Returns the count of rows removed.
    async fn revoke_all_for_subject(&self, subject: Subject) -> Result<usize, DomainError>;

    // ── Role-keyed grant operations ────────────────────────────────────────
    // These are the only grant write path. Lifecycle hook bulk-deletes
    // (`revoke_all_for_*` above) wipe matching rows directly, so callers
    // using those paths don't need to invoke `clear_role` separately.

    /// Set the role for a `(subject, resource)` pair. Idempotent via the
    /// UNIQUE `(subject_type, subject_id, resource_type, resource_id)`
    /// constraint — `ON CONFLICT` updates the role + expires_at if they
    /// changed, which is exactly the right semantics for an atomic role
    /// change (e.g. promoting Viewer → Editor in one UPDATE with no race
    /// window, no DELETE+INSERT).
    async fn set_role(
        &self,
        granted_by: Uuid,
        subject: Subject,
        role: Role,
        resource: Resource,
        expires_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<Grant, DomainError>;

    /// Remove the role for a `(subject, resource)` pair. Idempotent —
    /// succeeds whether or not the row existed. Called after `revoke`
    /// succeeds to keep the two tables in sync during dual-write; after
    /// cleanup this is the canonical role-revocation entry point.
    async fn clear_role(&self, subject: Subject, resource: Resource) -> Result<(), DomainError>;
}
