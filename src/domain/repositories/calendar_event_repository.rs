use crate::common::errors::DomainError;
use crate::domain::entities::calendar_event::CalendarEvent;
use chrono::{DateTime, Utc};
use uuid::Uuid;

pub type CalendarEventRepositoryResult<T> = Result<T, DomainError>;

/// Repository interface for CalendarEvent entity operations
pub trait CalendarEventRepository: Send + Sync + 'static {
    /// Creates a new calendar event
    async fn create_event(
        &self,
        event: CalendarEvent,
    ) -> CalendarEventRepositoryResult<CalendarEvent>;

    /// Updates an existing calendar event
    async fn update_event(
        &self,
        event: CalendarEvent,
    ) -> CalendarEventRepositoryResult<CalendarEvent>;

    /// Deletes a calendar event by ID
    async fn delete_event(&self, id: &Uuid) -> CalendarEventRepositoryResult<()>;

    /// Finds a calendar event by its ID
    async fn find_event_by_id(&self, id: &Uuid) -> CalendarEventRepositoryResult<CalendarEvent>;

    /// Narrow projection of `find_event_by_id` for authorization gates:
    /// just the owning `calendar_id`, without dragging the full row —
    /// notably `ical_data`, the raw iCalendar body — off the wire.
    async fn find_calendar_id_by_event_id(&self, id: &Uuid) -> CalendarEventRepositoryResult<Uuid>;

    /// Cursor stream over every event of `calendar_id` in bundle order:
    /// rows sorted by `(first occurrence per UID, uid, master-first,
    /// start_time)` so a recurring master + its exception overrides
    /// arrive adjacent and bundles appear in the first-appearance order
    /// the buffered `start_time` listing produced. ONE scan+sort on the
    /// server; the streaming CalDAV emitters cut pages at UID
    /// boundaries so only a page of rows is ever resident.
    fn stream_events_uid_order(
        &self,
        calendar_id: Uuid,
    ) -> futures::stream::BoxStream<'static, CalendarEventRepositoryResult<CalendarEvent>>;

    /// Lists all events in a specific calendar
    async fn list_events_by_calendar(
        &self,
        calendar_id: &Uuid,
    ) -> CalendarEventRepositoryResult<Vec<CalendarEvent>>;

    /// Finds events in a calendar by their summary/title (partial match)
    async fn find_events_by_summary(
        &self,
        calendar_id: &Uuid,
        summary: &str,
    ) -> CalendarEventRepositoryResult<Vec<CalendarEvent>>;

    /// Gets events in a specific time range for a calendar
    async fn get_events_in_time_range(
        &self,
        calendar_id: &Uuid,
        start: &DateTime<Utc>,
        end: &DateTime<Utc>,
    ) -> CalendarEventRepositoryResult<Vec<CalendarEvent>>;

    /// Finds an event by its iCalendar UID in a specific calendar.
    ///
    /// **Master-only lookup.** Filters `recurrence_id IS NULL` so the
    /// return value is unambiguous — the row that clients treat as
    /// "the event with this UID" is the master. Per-instance override
    /// rows share the UID but live under
    /// `find_event_by_ical_uid_and_recurrence_id` (see #528).
    async fn find_event_by_ical_uid(
        &self,
        calendar_id: &Uuid,
        ical_uid: &str,
    ) -> CalendarEventRepositoryResult<Option<CalendarEvent>>;

    /// Finds a specific per-instance exception override for a recurring
    /// master (RFC 5545 §3.8.4.4). `recurrence_id` pinpoints which
    /// occurrence of the master with the given UID is being targeted;
    /// returns `None` if no override has been PUT for that instance
    /// yet — which the PUT handler then uses to decide insert vs.
    /// update.
    ///
    /// The row is guaranteed unique by the partial index
    /// `idx_calendar_events_exception_unique`.
    async fn find_event_by_ical_uid_and_recurrence_id(
        &self,
        calendar_id: &Uuid,
        ical_uid: &str,
        recurrence_id: &DateTime<Utc>,
    ) -> CalendarEventRepositoryResult<Option<CalendarEvent>>;

    /// Finds the events matching any of the given iCalendar UIDs in one
    /// indexed query (`ical_uid = ANY(...)`). Used by CalDAV multiget so a
    /// request for a handful of events never pays for the whole calendar.
    /// UIDs with no matching event are silently absent from the result.
    async fn find_events_by_ical_uids(
        &self,
        calendar_id: &Uuid,
        ical_uids: &[String],
    ) -> CalendarEventRepositoryResult<Vec<CalendarEvent>>;

    /// Counts events in a calendar
    async fn count_events_in_calendar(
        &self,
        calendar_id: &Uuid,
    ) -> CalendarEventRepositoryResult<i64>;

    /// Deletes all events in a calendar
    async fn delete_all_events_in_calendar(
        &self,
        calendar_id: &Uuid,
    ) -> CalendarEventRepositoryResult<i64>;

    /// Lists events by calendar with pagination
    async fn list_events_by_calendar_paginated(
        &self,
        calendar_id: &Uuid,
        limit: i64,
        offset: i64,
    ) -> CalendarEventRepositoryResult<Vec<CalendarEvent>>;

    /// Finds events with recurrence rules that might occur in a time range
    async fn find_recurring_events_in_range(
        &self,
        calendar_id: &Uuid,
        start: &DateTime<Utc>,
        end: &DateTime<Utc>,
    ) -> CalendarEventRepositoryResult<Vec<CalendarEvent>>;
}
