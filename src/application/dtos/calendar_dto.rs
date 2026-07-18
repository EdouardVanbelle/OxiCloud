use crate::domain::entities::calendar::Calendar;
use crate::domain::entities::calendar_event::CalendarEvent;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// DTO for calendar data transfer
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CalendarDto {
    pub id: String,
    pub name: String,
    pub owner_id: String,
    pub description: Option<String>,
    pub color: Option<String>,
    pub is_public: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub custom_properties: HashMap<String, String>,
}

impl Default for CalendarDto {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            owner_id: String::new(),
            description: None,
            color: None,
            is_public: false,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            custom_properties: HashMap::new(),
        }
    }
}

impl From<Calendar> for CalendarDto {
    fn from(calendar: Calendar) -> Self {
        Self {
            id: calendar.id().to_string(),
            name: calendar.name().to_string(),
            owner_id: calendar.owner_id().to_string(),
            description: calendar.description().map(|s| s.to_string()),
            color: calendar.color().map(|s| s.to_string()),
            is_public: false, // This needs to be set separately as it's not part of the domain entity
            created_at: *calendar.created_at(),
            updated_at: *calendar.updated_at(),
            custom_properties: calendar.custom_properties().clone(),
        }
    }
}

/// DTO for calendar creation
#[derive(Debug, Serialize, Deserialize)]
pub struct CreateCalendarDto {
    pub name: String,
    pub description: Option<String>,
    pub color: Option<String>,
    pub is_public: Option<bool>,
}

/// DTO for calendar update
#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateCalendarDto {
    pub name: Option<String>,
    pub description: Option<String>,
    pub color: Option<String>,
    pub is_public: Option<bool>,
}

/// DTO for calendar sharing
#[derive(Debug, Serialize, Deserialize)]
pub struct CalendarShareDto {
    pub calendar_id: String,
    pub user_id: String,
    pub access_level: String, // 'read', 'write', 'owner'
}

/// DTO for calendar event data transfer
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CalendarEventDto {
    pub id: String,
    pub calendar_id: String,
    pub summary: String,
    pub description: Option<String>,
    pub location: Option<String>,
    pub start_time: DateTime<Utc>,
    pub end_time: DateTime<Utc>,
    pub all_day: bool,
    pub rrule: Option<String>,
    pub ical_uid: String,
    /// RFC 5545 §3.8.4.4 RECURRENCE-ID. `None` on masters and on
    /// non-recurring events; `Some` on per-instance exception
    /// overrides. Two rows sharing (`calendar_id`, `ical_uid`) but
    /// distinguished by this field represent a recurring master and
    /// its modified occurrence(s) respectively (see #528).
    pub recurrence_id: Option<DateTime<Utc>>,
    /// Full stored iCalendar body for this row — one VCALENDAR
    /// containing exactly one VEVENT. Populated at every read
    /// path from the entity's `ical_data()`. The CalDAV read
    /// emitters serve this verbatim (extracted + bundled per
    /// UID) instead of regenerating from the other DTO fields,
    /// so properties beyond the structured columns
    /// (ATTENDEE, VALARM, CATEGORIES, RECURRENCE-ID, X-*)
    /// survive PUT → GET round-trips. See phase-4 read-side
    /// unification.
    pub ical_data: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Default for CalendarEventDto {
    fn default() -> Self {
        Self {
            id: String::new(),
            calendar_id: String::new(),
            summary: String::new(),
            description: None,
            location: None,
            start_time: Utc::now(),
            end_time: Utc::now(),
            all_day: false,
            rrule: None,
            ical_uid: String::new(),
            recurrence_id: None,
            ical_data: String::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }
}

impl From<CalendarEvent> for CalendarEventDto {
    fn from(event: CalendarEvent) -> Self {
        // Move every owned field out of the consumed entity — the old
        // getter-clone shape deep-copied 6 Strings per event, dominated by
        // the ~11 KB `ical_data` blob, on every CalDAV listing row
        // (benches/ROUND11.md §19: 1.45x + the 11 KB memcpy gone).
        let parts = event.into_parts();
        Self {
            id: parts.id.to_string(),
            calendar_id: parts.calendar_id.to_string(),
            summary: parts.summary,
            description: parts.description,
            location: parts.location,
            start_time: parts.start_time,
            end_time: parts.end_time,
            all_day: parts.all_day,
            rrule: parts.rrule,
            ical_uid: parts.ical_uid,
            recurrence_id: parts.recurrence_id,
            ical_data: parts.ical_data,
            created_at: parts.created_at,
            updated_at: parts.updated_at,
        }
    }
}

/// DTO for calendar event creation using iCalendar data
#[derive(Debug, Serialize, Deserialize)]
pub struct CreateEventICalDto {
    pub calendar_id: String,
    pub ical_data: String,
}

/// DTO for calendar event creation with structured data
#[derive(Debug, Serialize, Deserialize)]
pub struct CreateEventDto {
    pub calendar_id: String,
    pub summary: String,
    pub description: Option<String>,
    pub location: Option<String>,
    pub start_time: DateTime<Utc>,
    pub end_time: DateTime<Utc>,
    pub all_day: Option<bool>,
    pub rrule: Option<String>,
    pub user_id: String, // Added for authorization
}

/// DTO for updating a calendar event
#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateEventDto {
    pub summary: Option<String>,
    pub description: Option<String>,
    pub location: Option<String>,
    pub start_time: Option<DateTime<Utc>>,
    pub end_time: Option<DateTime<Utc>>,
    pub all_day: Option<bool>,
    pub rrule: Option<String>,
    pub user_id: String, // Added for authorization
}

/// DTO for querying events in a time range
#[derive(Debug, Serialize, Deserialize)]
pub struct EventQueryDto {
    pub calendar_id: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

/// DTO for pagination
#[derive(Debug, Serialize, Deserialize)]
pub struct PaginationDto {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}
