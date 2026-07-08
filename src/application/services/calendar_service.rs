use chrono::{DateTime, Utc};
use std::collections::HashSet;
use std::sync::Arc;
use uuid::Uuid;

use crate::application::dtos::calendar_dto::{
    CalendarDto, CalendarEventDto, CreateCalendarDto, CreateEventDto, CreateEventICalDto,
    UpdateCalendarDto, UpdateEventDto,
};
use crate::application::ports::authorization_ports::AuthorizationEngine;
use crate::application::ports::calendar_ports::{CalendarStoragePort, CalendarUseCase};
use crate::common::errors::{DomainError, ErrorKind};
use crate::domain::services::authorization::{Permission, Resource, Role, Subject};
use crate::infrastructure::adapters::calendar_storage_adapter::CalendarStorageAdapter;
use crate::infrastructure::services::pg_acl_engine::PgAclEngine;

/// Calendar service — the CalDAV / REST entry point for every calendar
/// or event operation. Every method routes through `AuthorizationEngine`;
/// the pre-Round-3 `check_calendar_access` bespoke helper is gone.
///
/// Ownership + sharing live entirely in `storage.role_grants`
/// (`resource_type='calendar'`). `caldav.calendars.owner_id` stays for
/// provenance and legacy queries but is no longer consulted for access
/// decisions.
pub struct CalendarService {
    calendar_storage: Arc<CalendarStorageAdapter>,
    /// ReBAC engine — every user-facing method calls `authz.require`
    /// with the appropriate `Permission`. `create_calendar` also
    /// uses it to seed an Owner grant for the caller so the common
    /// "owning my own calendar" case takes a single indexed
    /// role_grants lookup.
    authz: Arc<PgAclEngine>,
}

impl CalendarService {
    pub fn new(calendar_storage: Arc<CalendarStorageAdapter>, authz: Arc<PgAclEngine>) -> Self {
        Self {
            calendar_storage,
            authz,
        }
    }

    /// Parse `calendar_id` and enforce `permission` on `Resource::Calendar(uuid)`.
    /// On denial `authz.require` returns `NotFound` (anti-enum — same
    /// shape as "no such calendar") and emits the `authz.denied` audit
    /// line. Returns the parsed UUID on success so the caller doesn't
    /// have to parse it a second time.
    async fn require_calendar_perm(
        &self,
        calendar_id: &str,
        caller_id: Uuid,
        permission: Permission,
    ) -> Result<Uuid, DomainError> {
        let uuid = Uuid::parse_str(calendar_id)
            .map_err(|_| DomainError::new(ErrorKind::InvalidInput, "Calendar", "Invalid ID"))?;
        self.authz
            .require(
                Subject::User(caller_id),
                permission,
                Resource::Calendar(uuid),
            )
            .await?;
        Ok(uuid)
    }

    /// Check `permission` on a calendar without throwing. Used by the
    /// read paths that also allow a public-calendar bypass — they need
    /// a bool, not a `Result<(), NotFound>`.
    async fn has_calendar_perm(
        &self,
        calendar_id: &str,
        caller_id: Uuid,
        permission: Permission,
    ) -> Result<bool, DomainError> {
        let uuid = Uuid::parse_str(calendar_id)
            .map_err(|_| DomainError::new(ErrorKind::InvalidInput, "Calendar", "Invalid ID"))?;
        self.authz
            .check(
                Subject::User(caller_id),
                permission,
                Resource::Calendar(uuid),
            )
            .await
    }
}

impl CalendarUseCase for CalendarService {
    async fn create_calendar(
        &self,
        calendar: CreateCalendarDto,
        user_id: Uuid,
    ) -> Result<CalendarDto, DomainError> {
        // No pre-write gate: creating a calendar is a personal act
        // (like creating a folder in your own drive). Storage stamps
        // `owner_id = user_id`; we then seed an Owner role_grant so
        // the engine's cache warms on first-read.
        let created = self
            .calendar_storage
            .create_calendar(calendar, user_id)
            .await?;
        let calendar_uuid = Uuid::parse_str(&created.id).map_err(|_| {
            DomainError::internal_error("Calendar", "storage returned invalid calendar id")
        })?;
        // `set_role` is idempotent on the `(subject, resource)` unique
        // key — a re-run (rare — only if storage retried) is a no-op.
        // `granted_by = user_id` is the self-seeded creation event.
        self.authz
            .set_role(
                user_id,
                Subject::User(user_id),
                Role::Owner,
                Resource::Calendar(calendar_uuid),
                None,
            )
            .await?;
        Ok(created)
    }

    async fn update_calendar(
        &self,
        calendar_id: &str,
        update: UpdateCalendarDto,
        user_id: Uuid,
    ) -> Result<CalendarDto, DomainError> {
        self.require_calendar_perm(calendar_id, user_id, Permission::Update)
            .await?;
        self.calendar_storage
            .update_calendar(calendar_id, update)
            .await
    }

    async fn delete_calendar(&self, calendar_id: &str, user_id: Uuid) -> Result<(), DomainError> {
        let uuid = self
            .require_calendar_perm(calendar_id, user_id, Permission::Delete)
            .await?;
        self.calendar_storage.delete_calendar(calendar_id).await?;
        // Wipe every grant on this calendar so a re-used UUID (impossible
        // today but cheap to defend against) doesn't inherit stale ACLs.
        // The storage DELETE won't cascade to `storage.role_grants` — the
        // legacy `caldav.calendar_shares` had an FK, `role_grants`
        // doesn't (it's cross-schema).
        let _ = self
            .authz
            .revoke_all_for_resource(Resource::Calendar(uuid))
            .await;
        Ok(())
    }

    async fn get_calendar(
        &self,
        calendar_id: &str,
        user_id: Uuid,
    ) -> Result<CalendarDto, DomainError> {
        let calendar = self.calendar_storage.get_calendar(calendar_id).await?;
        // Public-calendar bypass: anonymous-ish read. `check` returns
        // bool (no throw); combine with the public flag before
        // deciding.
        let allowed = calendar.is_public
            || self
                .has_calendar_perm(calendar_id, user_id, Permission::Read)
                .await?;
        if !allowed {
            return Err(DomainError::not_found("Calendar", calendar_id));
        }
        Ok(calendar)
    }

    async fn list_my_calendars(&self, user_id: Uuid) -> Result<Vec<CalendarDto>, DomainError> {
        // Post-Round-3 semantics: every calendar the caller has any
        // grant on — owned + shared, one union. The pre-Round-3
        // `list_calendars_by_owner` returned owner-only; shared
        // calendars never surfaced through this method. See
        // `docs/plan/caldav-carddav-migration-to-authz.md`.
        let grants = self
            .authz
            .list_incoming_grants(Subject::User(user_id))
            .await?;

        // Deduplicate — a user can hold multiple grants on the same
        // calendar (direct + group-inherited). We only need one DTO
        // per resource.
        let calendar_ids: HashSet<Uuid> = grants
            .into_iter()
            .filter_map(|g| match g.resource {
                Resource::Calendar(id) => Some(id),
                _ => None,
            })
            .collect();

        // Hydrate DTOs. `get_calendar` misses on trashed / deleted
        // calendars — those are dropped from the listing rather than
        // erroring, so a lifecycle-race doesn't turn a PROPFIND into
        // a 5xx.
        let mut out = Vec::with_capacity(calendar_ids.len());
        for id in calendar_ids {
            if let Ok(dto) = self.calendar_storage.get_calendar(&id.to_string()).await {
                out.push(dto);
            }
        }
        Ok(out)
    }

    async fn list_public_calendars(
        &self,
        limit: Option<i64>,
        offset: Option<i64>,
    ) -> Result<Vec<CalendarDto>, DomainError> {
        // No caller gate: public listing by definition. Storage
        // filters on `is_public = true`.
        let limit = limit.unwrap_or(100);
        let offset = offset.unwrap_or(0);
        self.calendar_storage
            .list_public_calendars(limit, offset)
            .await
    }

    async fn create_event(
        &self,
        event: CreateEventDto,
        user_id: Uuid,
    ) -> Result<CalendarEventDto, DomainError> {
        self.require_calendar_perm(&event.calendar_id, user_id, Permission::Create)
            .await?;
        self.calendar_storage.create_event(event).await
    }

    async fn create_event_from_ical(
        &self,
        event: CreateEventICalDto,
        user_id: Uuid,
    ) -> Result<CalendarEventDto, DomainError> {
        self.require_calendar_perm(&event.calendar_id, user_id, Permission::Create)
            .await?;
        self.calendar_storage.create_event_from_ical(event).await
    }

    async fn update_event(
        &self,
        event_id: &str,
        update: UpdateEventDto,
        user_id: Uuid,
    ) -> Result<CalendarEventDto, DomainError> {
        let event = self.calendar_storage.get_event(event_id).await?;
        self.require_calendar_perm(&event.calendar_id, user_id, Permission::Update)
            .await?;
        self.calendar_storage.update_event(event_id, update).await
    }

    async fn delete_event(&self, event_id: &str, user_id: Uuid) -> Result<(), DomainError> {
        let event = self.calendar_storage.get_event(event_id).await?;
        self.require_calendar_perm(&event.calendar_id, user_id, Permission::Delete)
            .await?;
        self.calendar_storage.delete_event(event_id).await
    }

    async fn get_event(
        &self,
        event_id: &str,
        user_id: Uuid,
    ) -> Result<CalendarEventDto, DomainError> {
        let event = self.calendar_storage.get_event(event_id).await?;
        let calendar = self
            .calendar_storage
            .get_calendar(&event.calendar_id)
            .await?;
        // Same public-calendar bypass as `get_calendar`.
        let allowed = calendar.is_public
            || self
                .has_calendar_perm(&event.calendar_id, user_id, Permission::Read)
                .await?;
        if !allowed {
            return Err(DomainError::not_found("Event", event_id));
        }
        Ok(event)
    }

    async fn get_event_by_ical_uid(
        &self,
        calendar_id: &str,
        ical_uid: &str,
        user_id: Uuid,
    ) -> Result<Option<CalendarEventDto>, DomainError> {
        let calendar = self.calendar_storage.get_calendar(calendar_id).await?;
        let allowed = calendar.is_public
            || self
                .has_calendar_perm(calendar_id, user_id, Permission::Read)
                .await?;
        if !allowed {
            return Err(DomainError::not_found("Calendar", calendar_id));
        }
        self.calendar_storage
            .find_event_by_ical_uid(calendar_id, ical_uid)
            .await
    }

    async fn get_events_by_ical_uids(
        &self,
        calendar_id: &str,
        ical_uids: &[String],
        user_id: Uuid,
    ) -> Result<Vec<CalendarEventDto>, DomainError> {
        let calendar = self.calendar_storage.get_calendar(calendar_id).await?;
        let allowed = calendar.is_public
            || self
                .has_calendar_perm(calendar_id, user_id, Permission::Read)
                .await?;
        if !allowed {
            return Err(DomainError::not_found("Calendar", calendar_id));
        }
        if ical_uids.is_empty() {
            return Ok(Vec::new());
        }
        self.calendar_storage
            .find_events_by_ical_uids(calendar_id, ical_uids)
            .await
    }

    async fn list_events(
        &self,
        calendar_id: &str,
        limit: Option<i64>,
        offset: Option<i64>,
        user_id: Uuid,
    ) -> Result<Vec<CalendarEventDto>, DomainError> {
        let calendar = self.calendar_storage.get_calendar(calendar_id).await?;
        let allowed = calendar.is_public
            || self
                .has_calendar_perm(calendar_id, user_id, Permission::Read)
                .await?;
        if !allowed {
            return Err(DomainError::not_found("Calendar", calendar_id));
        }
        if limit.is_some() || offset.is_some() {
            let limit = limit.unwrap_or(100);
            let offset = offset.unwrap_or(0);
            self.calendar_storage
                .list_events_by_calendar_paginated(calendar_id, limit, offset)
                .await
        } else {
            self.calendar_storage
                .list_events_by_calendar(calendar_id)
                .await
        }
    }

    async fn get_events_in_range(
        &self,
        calendar_id: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        user_id: Uuid,
    ) -> Result<Vec<CalendarEventDto>, DomainError> {
        let calendar = self.calendar_storage.get_calendar(calendar_id).await?;
        let allowed = calendar.is_public
            || self
                .has_calendar_perm(calendar_id, user_id, Permission::Read)
                .await?;
        if !allowed {
            return Err(DomainError::not_found("Calendar", calendar_id));
        }
        self.calendar_storage
            .get_events_in_time_range(calendar_id, &start, &end)
            .await
    }
}
