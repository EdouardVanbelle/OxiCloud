use chrono::{DateTime, Duration, TimeZone, Utc};
/**
 * Calendar Event Entity
 *
 * This module defines the CalendarEvent entity, which represents an event or
 * appointment in a calendar, following the iCalendar (RFC 5545) specification.
 *
 * Calendar events have properties like summary, description, location, start/end times,
 * and can include recurrence rules for repeating events. Each event belongs to a
 * specific calendar and stores its complete iCalendar representation.
 */
use uuid::Uuid;

use crate::common::errors::{DomainError, ErrorKind, Result};

// Re-export entity errors from the centralized module
pub use super::entity_errors::CalendarEventError;

/**
 * CalendarEvent entity.
 *
 * Represents a calendar event or appointment that can be synced via CalDAV.
 * Follows the iCalendar format (RFC 5545) for compatibility with CalDAV clients.
 */
#[derive(Debug, Clone)]
pub struct CalendarEvent {
    /// Unique identifier for the event
    id: Uuid,

    /// ID of the calendar this event belongs to
    calendar_id: Uuid,

    /// Short summary/title of the event
    summary: String,

    /// Detailed description of the event (optional)
    description: Option<String>,

    /// Location of the event (optional)
    location: Option<String>,

    /// Start time of the event
    start_time: DateTime<Utc>,

    /// End time of the event
    end_time: DateTime<Utc>,

    /// Whether this is an all-day event
    all_day: bool,

    /// Recurrence rule in iCalendar RRULE format (optional)
    rrule: Option<String>,

    /// Unique identifier in iCalendar format (used for CalDAV sync)
    ical_uid: String,

    /// Complete iCalendar data (VEVENT component)
    ical_data: String,

    /// Time when the event was created
    created_at: DateTime<Utc>,

    /// Time when the event was last modified
    updated_at: DateTime<Utc>,
}

impl CalendarEvent {
    /**
     * Creates a new calendar event with the given properties.
     *
     * @param calendar_id ID of the calendar this event belongs to
     * @param summary Short summary/title of the event
     * @param description Detailed description of the event (optional)
     * @param location Location of the event (optional)
     * @param start_time Start time of the event
     * @param end_time End time of the event
     * @param all_day Whether this is an all-day event
     * @param rrule Recurrence rule in iCalendar RRULE format (optional)
     * @param ical_data Complete iCalendar data (VEVENT component)
     * @return Result containing the new CalendarEvent or a domain error
     */
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        calendar_id: Uuid,
        summary: String,
        description: Option<String>,
        location: Option<String>,
        start_time: DateTime<Utc>,
        end_time: DateTime<Utc>,
        all_day: bool,
        rrule: Option<String>,
        ical_data: String,
    ) -> Result<Self> {
        // Validate inputs
        if summary.is_empty() {
            return Err(DomainError::new(
                ErrorKind::InvalidInput,
                "CalendarEvent",
                "Event summary cannot be empty",
            ));
        }

        if end_time < start_time {
            return Err(DomainError::new(
                ErrorKind::InvalidInput,
                "CalendarEvent",
                "End time cannot be before start time",
            ));
        }

        // Validate RRULE if provided (basic validation)
        if let Some(ref rule) = rrule
            && !rule.starts_with("FREQ=")
        {
            return Err(DomainError::new(
                ErrorKind::InvalidInput,
                "CalendarEvent",
                "Recurrence rule must start with FREQ=",
            ));
        }

        // Validate iCalendar data (basic validation)
        if !ical_data.contains("BEGIN:VEVENT") || !ical_data.contains("END:VEVENT") {
            return Err(DomainError::new(
                ErrorKind::InvalidInput,
                "CalendarEvent",
                "iCalendar data must contain a VEVENT component",
            ));
        }

        let now = Utc::now();

        Ok(Self {
            id: Uuid::new_v4(),
            calendar_id,
            summary,
            description,
            location,
            start_time,
            end_time,
            all_day,
            rrule,
            ical_uid: Uuid::new_v4().to_string(),
            ical_data,
            created_at: now,
            updated_at: now,
        })
    }

    /**
     * Creates a calendar event with specific ID and timestamps.
     * Typically used when reconstructing from storage.
     *
     * @param id Unique identifier for the event
     * @param calendar_id ID of the calendar this event belongs to
     * @param summary Short summary/title of the event
     * @param description Detailed description of the event (optional)
     * @param location Location of the event (optional)
     * @param start_time Start time of the event
     * @param end_time End time of the event
     * @param all_day Whether this is an all-day event
     * @param rrule Recurrence rule in iCalendar RRULE format (optional)
     * @param ical_uid Unique identifier in iCalendar format
     * @param ical_data Complete iCalendar data (VEVENT component)
     * @param created_at Time when the event was created
     * @param updated_at Time when the event was last modified
     * @return Result containing the new CalendarEvent or a domain error
     */
    #[allow(clippy::too_many_arguments)]
    pub fn with_id(
        id: Uuid,
        calendar_id: Uuid,
        summary: String,
        description: Option<String>,
        location: Option<String>,
        start_time: DateTime<Utc>,
        end_time: DateTime<Utc>,
        all_day: bool,
        rrule: Option<String>,
        ical_uid: String,
        ical_data: String,
        created_at: DateTime<Utc>,
        updated_at: DateTime<Utc>,
    ) -> Result<Self> {
        // Basic validation
        if summary.is_empty() {
            return Err(DomainError::new(
                ErrorKind::InvalidInput,
                "CalendarEvent",
                "Event summary cannot be empty",
            ));
        }

        if end_time < start_time {
            return Err(DomainError::new(
                ErrorKind::InvalidInput,
                "CalendarEvent",
                "End time cannot be before start time",
            ));
        }

        Ok(Self {
            id,
            calendar_id,
            summary,
            description,
            location,
            start_time,
            end_time,
            all_day,
            rrule,
            ical_uid,
            ical_data,
            created_at,
            updated_at,
        })
    }

    /**
     * Creates a calendar event from an iCalendar VEVENT component.
     * Parses the iCalendar data to extract event properties.
     *
     * @param calendar_id ID of the calendar this event belongs to
     * @param ical_data Complete iCalendar data (VEVENT component)
     * @return Result containing the new CalendarEvent or a domain error
     */
    pub fn from_ical(calendar_id: Uuid, ical_data: String) -> Result<Self> {
        // This implementation would require a proper iCalendar parser
        // For brevity, we're using a simplified version here

        // Extract required fields from iCalendar data
        let summary = Self::extract_ical_property(&ical_data, "SUMMARY").ok_or_else(|| {
            DomainError::new(
                ErrorKind::InvalidInput,
                "CalendarEvent",
                "Missing SUMMARY in iCalendar data",
            )
        })?;

        // DTSTART / DTEND: use the params-aware extractor so we can
        // detect `VALUE=DATE` (all-day) from the property parameters
        // rather than scanning the raw property line. The pre-parser-
        // rewrite substring scan couldn't see param-carrying lines at
        // all — see #528.
        let (dtstart_value, dtstart_params) =
            Self::extract_ical_property_with_params(&ical_data, "DTSTART").ok_or_else(|| {
                DomainError::new(
                    ErrorKind::InvalidInput,
                    "CalendarEvent",
                    "Missing DTSTART in iCalendar data",
                )
            })?;

        let (dtend_value, _dtend_params) =
            Self::extract_ical_property_with_params(&ical_data, "DTEND").ok_or_else(|| {
                DomainError::new(
                    ErrorKind::InvalidInput,
                    "CalendarEvent",
                    "Missing DTEND in iCalendar data",
                )
            })?;

        // All-day detection: `VALUE=DATE` parameter on DTSTART.
        // Falls back to `false` when the parameter is absent, matching
        // RFC 5545 §3.3.4 ("If the property permits, multiple 'VALUE'
        // parameters can be specified as a comma-separated list") —
        // we're strict: only "DATE" (case-insensitive) counts, "DATE-TIME"
        // and anything else means timed.
        let all_day = dtstart_params
            .get("VALUE")
            .map(|vs| {
                vs.iter()
                    .any(|v| v.eq_ignore_ascii_case("DATE"))
            })
            .unwrap_or(false);

        let start_time = Self::parse_ical_datetime(&dtstart_value, all_day).map_err(|e| {
            DomainError::new(
                ErrorKind::InvalidInput,
                "CalendarEvent",
                format!("Invalid DTSTART: {}", e),
            )
        })?;

        let end_time = Self::parse_ical_datetime(&dtend_value, all_day).map_err(|e| {
            DomainError::new(
                ErrorKind::InvalidInput,
                "CalendarEvent",
                format!("Invalid DTEND: {}", e),
            )
        })?;

        // Extract optional fields
        let description = Self::extract_ical_property(&ical_data, "DESCRIPTION");
        let location = Self::extract_ical_property(&ical_data, "LOCATION");
        let rrule = Self::extract_ical_property(&ical_data, "RRULE");

        // Extract UID or generate a new one
        let ical_uid = Self::extract_ical_property(&ical_data, "UID")
            .unwrap_or_else(|| Uuid::new_v4().to_string());

        let now = Utc::now();

        Ok(Self {
            id: Uuid::new_v4(),
            calendar_id,
            summary,
            description,
            location,
            start_time,
            end_time,
            all_day,
            rrule,
            ical_uid,
            ical_data,
            created_at: now,
            updated_at: now,
        })
    }

    // Getters

    /// Returns the event's unique identifier
    pub fn id(&self) -> &Uuid {
        &self.id
    }

    /// Returns the ID of the calendar this event belongs to
    pub fn calendar_id(&self) -> &Uuid {
        &self.calendar_id
    }

    /// Returns the event's summary/title
    pub fn summary(&self) -> &str {
        &self.summary
    }

    /// Returns the event's description, if any
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    /// Returns the event's location, if any
    pub fn location(&self) -> Option<&str> {
        self.location.as_deref()
    }

    /// Returns the event's start time
    pub fn start_time(&self) -> &DateTime<Utc> {
        &self.start_time
    }

    /// Returns the event's end time
    pub fn end_time(&self) -> &DateTime<Utc> {
        &self.end_time
    }

    /// Returns whether this is an all-day event
    pub fn all_day(&self) -> bool {
        self.all_day
    }

    /// Returns the event's recurrence rule, if any
    pub fn rrule(&self) -> Option<&str> {
        self.rrule.as_deref()
    }

    /// Returns the event's iCalendar UID
    pub fn ical_uid(&self) -> &str {
        &self.ical_uid
    }

    /// Returns the complete iCalendar data for the event
    pub fn ical_data(&self) -> &str {
        &self.ical_data
    }

    /// Returns the time when the event was created
    pub fn created_at(&self) -> &DateTime<Utc> {
        &self.created_at
    }

    /// Returns the time when the event was last modified
    pub fn updated_at(&self) -> &DateTime<Utc> {
        &self.updated_at
    }

    /// Returns the duration of the event
    pub fn duration(&self) -> Duration {
        self.end_time - self.start_time
    }

    // Setters and Mutators

    /**
     * Updates the event's summary/title.
     *
     * @param summary New summary/title for the event
     * @return Result indicating success or containing a domain error
     */
    pub fn update_summary(&mut self, summary: String) -> Result<()> {
        if summary.is_empty() {
            return Err(DomainError::new(
                ErrorKind::InvalidInput,
                "CalendarEvent",
                "Event summary cannot be empty",
            ));
        }

        // Clone the summary before updating the struct
        let summary_clone = summary.clone();
        self.summary = summary;
        self.updated_at = Utc::now();

        // Update iCalendar data using the cloned value
        self.update_ical_property("SUMMARY", &summary_clone);

        Ok(())
    }

    /**
     * Updates the event's description.
     *
     * @param description New description for the event
     */
    pub fn update_description(&mut self, description: Option<String>) {
        self.description = description.clone();
        self.updated_at = Utc::now();

        // Update iCalendar data
        match description {
            Some(desc) => self.update_ical_property("DESCRIPTION", &desc),
            None => self.remove_ical_property("DESCRIPTION"),
        }
    }

    /**
     * Updates the event's location.
     *
     * @param location New location for the event
     */
    pub fn update_location(&mut self, location: Option<String>) {
        self.location = location.clone();
        self.updated_at = Utc::now();

        // Update iCalendar data
        match location {
            Some(loc) => self.update_ical_property("LOCATION", &loc),
            None => self.remove_ical_property("LOCATION"),
        }
    }

    /**
     * Updates the event's start and end times.
     *
     * @param start_time New start time for the event
     * @param end_time New end time for the event
     * @return Result indicating success or containing a domain error
     */
    pub fn update_time_range(
        &mut self,
        start_time: DateTime<Utc>,
        end_time: DateTime<Utc>,
    ) -> Result<()> {
        if end_time < start_time {
            return Err(DomainError::new(
                ErrorKind::InvalidInput,
                "CalendarEvent",
                "End time cannot be before start time",
            ));
        }

        self.start_time = start_time;
        self.end_time = end_time;
        self.updated_at = Utc::now();

        // Update iCalendar data
        let start_str = if self.all_day {
            format!("{}T000000Z", start_time.format("%Y%m%d"))
        } else {
            format!("{}", start_time.format("%Y%m%dT%H%M%SZ"))
        };

        let end_str = if self.all_day {
            format!("{}T000000Z", end_time.format("%Y%m%d"))
        } else {
            format!("{}", end_time.format("%Y%m%dT%H%M%SZ"))
        };

        self.update_ical_property("DTSTART", &start_str);
        self.update_ical_property("DTEND", &end_str);

        Ok(())
    }

    /**
     * Updates whether this is an all-day event.
     *
     * @param all_day Whether this is an all-day event
     */
    pub fn update_all_day(&mut self, all_day: bool) {
        self.all_day = all_day;
        self.updated_at = Utc::now();

        // Update iCalendar data
        let start_str = if all_day {
            format!("VALUE=DATE:{}", self.start_time.format("%Y%m%d"))
        } else {
            format!("{}", self.start_time.format("%Y%m%dT%H%M%SZ"))
        };

        let end_str = if all_day {
            format!("VALUE=DATE:{}", self.end_time.format("%Y%m%d"))
        } else {
            format!("{}", self.end_time.format("%Y%m%dT%H%M%SZ"))
        };

        self.update_ical_property("DTSTART", &start_str);
        self.update_ical_property("DTEND", &end_str);
    }

    /**
     * Updates the event's recurrence rule.
     *
     * @param rrule New recurrence rule for the event
     * @return Result indicating success or containing a domain error
     */
    pub fn update_rrule(&mut self, rrule: Option<String>) -> Result<()> {
        // Validate RRULE if provided (basic validation)
        if let Some(ref rule) = rrule
            && !rule.starts_with("FREQ=")
        {
            return Err(DomainError::new(
                ErrorKind::InvalidInput,
                "CalendarEvent",
                "Recurrence rule must start with FREQ=",
            ));
        }

        self.rrule = rrule.clone();
        self.updated_at = Utc::now();

        // Update iCalendar data
        match rrule {
            Some(rule) => self.update_ical_property("RRULE", &rule),
            None => self.remove_ical_property("RRULE"),
        }

        Ok(())
    }

    /**
     * Updates the complete iCalendar data for the event.
     * Also updates the event properties based on the new iCalendar data.
     *
     * @param ical_data New iCalendar data for the event
     * @return Result indicating success or containing a domain error
     */
    pub fn update_ical_data(&mut self, ical_data: String) -> Result<()> {
        // Validate iCalendar data (basic validation)
        if !ical_data.contains("BEGIN:VEVENT") || !ical_data.contains("END:VEVENT") {
            return Err(DomainError::new(
                ErrorKind::InvalidInput,
                "CalendarEvent",
                "iCalendar data must contain a VEVENT component",
            ));
        }

        // Extract and update properties from iCalendar data
        if let Some(summary) = Self::extract_ical_property(&ical_data, "SUMMARY") {
            self.summary = summary;
        }

        self.description = Self::extract_ical_property(&ical_data, "DESCRIPTION");
        self.location = Self::extract_ical_property(&ical_data, "LOCATION");

        // Extract DTSTART with parameters — needed for the all-day
        // detection below AND for the DTSTART/DTEND datetime parsers
        // (they need to know whether the value is a date or a datetime).
        let dtstart_pair = Self::extract_ical_property_with_params(&ical_data, "DTSTART");
        let all_day = dtstart_pair
            .as_ref()
            .and_then(|(_v, params)| params.get("VALUE"))
            .map(|vs| vs.iter().any(|v| v.eq_ignore_ascii_case("DATE")))
            .unwrap_or(false);
        self.all_day = all_day;

        if let Some((value, _params)) = &dtstart_pair
            && let Ok(start_time) = Self::parse_ical_datetime(value, all_day)
        {
            self.start_time = start_time;
        }

        if let Some((value, _params)) =
            Self::extract_ical_property_with_params(&ical_data, "DTEND")
            && let Ok(end_time) = Self::parse_ical_datetime(&value, all_day)
        {
            self.end_time = end_time;
        }

        self.rrule = Self::extract_ical_property(&ical_data, "RRULE");

        if let Some(uid) = Self::extract_ical_property(&ical_data, "UID") {
            self.ical_uid = uid;
        }

        self.ical_data = ical_data;
        self.updated_at = Utc::now();

        Ok(())
    }

    /**
     * Checks if this event belongs to the specified calendar.
     *
     * @param calendar_id ID of the calendar to check against
     * @return true if the event belongs to the calendar, false otherwise
     */
    pub fn belongs_to_calendar(&self, calendar_id: &Uuid) -> bool {
        self.calendar_id == *calendar_id
    }

    /**
     * Checks if this event occurs within the specified time range.
     *
     * @param start Start of the time range to check
     * @param end End of the time range to check
     * @return true if the event occurs within the range, false otherwise
     */
    pub fn occurs_in_range(&self, start: &DateTime<Utc>, end: &DateTime<Utc>) -> bool {
        // Basic case: event directly overlaps with range
        if self.start_time <= *end && self.end_time >= *start {
            return true;
        }

        // If event has recurrence, check if any recurrence occurs in range
        // Note: A full implementation would need a proper recurrence rule parser
        if let Some(rrule) = &self.rrule {
            // Simplified check for demonstration
            // A real implementation would need to generate recurrence instances
            // and check if any fall within the range

            // For now, we'll just check if the recurrence hasn't ended
            // or if it ended after the start of our range
            if let Some(until_pos) = rrule.find("UNTIL=") {
                let until_start = until_pos + 6; // "UNTIL=" is 6 chars
                let until_str = if let Some(until_end) = rrule[until_start..].find(';') {
                    &rrule[until_start..until_start + until_end]
                } else {
                    // UNTIL is the last part of the rule
                    &rrule[until_start..]
                };
                // RFC 5545 §3.3.10 — UNTIL is either a DATE (`YYYYMMDD`,
                // 8 chars) or a DATE-TIME (`YYYYMMDDTHHMMSSZ`, 16 chars,
                // trailing Z). Distinguish by shape: exactly 8 chars ⇒
                // date-only. Everything else is treated as datetime and
                // parsed accordingly.
                let is_date_only = until_str.len() == 8;
                if let Ok(until_date) = Self::parse_ical_datetime(until_str, is_date_only) {
                    return until_date >= *start;
                }
            } else {
                // No UNTIL specified, so recurrence continues indefinitely
                return true;
            }
        }

        false
    }

    // Helper methods for iCalendar operations

    /**
     * Extracts a property value from iCalendar data.
     *
     * Backed by the `ical` crate's RFC 5545 parser (see `Cargo.toml`
     * doc-comment on the dep). The pre-2026-07-14 hand-rolled scan
     * looked for `\n<NAME>:` and refused any parameter-carrying
     * property (`DTSTART;VALUE=DATE:20260101`,
     * `RECURRENCE-ID;VALUE=DATE:...`, `ATTENDEE;CN=…;PARTSTAT=…:…`) —
     * see AtalayaLabs/OxiCloud#528.
     *
     * The current implementation reads the first VEVENT from the raw
     * body via `IcalParser` and returns the named property's `value`
     * (parameters discarded — use `extract_ical_property_with_params`
     * for callers that care about `VALUE=DATE`, `TZID`, etc.).
     *
     * Returns `None` when the property is missing, has an empty value,
     * or the body isn't parseable as iCalendar. Whole-body parse
     * failures collapse to `None` rather than surface — same behaviour
     * as the pre-rewrite hand-rolled scan, which just returned `None`
     * on any mismatch. If callers need to distinguish "missing" from
     * "unparseable body", they should use `parse_first_vevent` directly.
     *
     * @param ical_data The iCalendar data to search in
     * @param property_name The name of the property to extract
     * @return Option containing the property value if found
     */
    fn extract_ical_property(ical_data: &str, property_name: &str) -> Option<String> {
        Self::extract_ical_property_with_params(ical_data, property_name).map(|(v, _p)| v)
    }

    /// Extract a property's value AND parameter map. Same lookup rules
    /// as `extract_ical_property`; the second element is a map keyed by
    /// parameter name (`"VALUE"`, `"TZID"`, `"CN"`, …) whose value is
    /// the list of parameter values (parameters can be multi-valued —
    /// `MEMBER="mailto:a@x","mailto:b@x"` — hence the `Vec<String>`
    /// per key).
    ///
    /// Callers that only need the value should use `extract_ical_property`;
    /// this variant is for DTSTART / DTEND / RECURRENCE-ID which need
    /// `VALUE=DATE` detection to distinguish all-day from timed events.
    fn extract_ical_property_with_params(
        ical_data: &str,
        property_name: &str,
    ) -> Option<(String, std::collections::HashMap<String, Vec<String>>)> {
        let event = Self::parse_first_vevent(ical_data)?;
        let prop = event
            .properties
            .into_iter()
            .find(|p| p.name.eq_ignore_ascii_case(property_name))?;
        let value = prop.value?;
        if value.trim().is_empty() {
            return None;
        }
        let mut params: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        if let Some(param_list) = prop.params {
            for (name, values) in param_list {
                // RFC 5545 property parameter names are ASCII case-insensitive.
                // Normalise to UPPER so callers key on a canonical form.
                params.insert(name.to_ascii_uppercase(), values);
            }
        }
        Some((value.trim().to_string(), params))
    }

    /// Parse the raw iCalendar body and return the first VEVENT
    /// component's properties. Returns `None` on any parse failure or
    /// if the body carries zero events (e.g. a `VCALENDAR` with only
    /// VTODOs — not our concern for the events surface).
    ///
    /// Delegated to the `ical` crate's `IcalParser`, which handles
    /// line-folding, escaped characters, and RFC 5545 parameter syntax.
    fn parse_first_vevent(ical_data: &str) -> Option<ical::parser::ical::component::IcalEvent> {
        use std::io::BufReader;
        let reader = BufReader::new(ical_data.as_bytes());
        let parser = ical::IcalParser::new(reader);
        for cal in parser {
            let Ok(cal) = cal else { continue };
            if let Some(event) = cal.events.into_iter().next() {
                return Some(event);
            }
        }
        None
    }

    /**
     * Parses an iCalendar datetime string into a DateTime object.
     *
     * @param value The property value (already stripped of parameters
     *              by the ical-crate-backed extractor).
     * @param is_date_only True when the source line carried
     *                     `VALUE=DATE` (all-day event) — caller derives
     *                     this from `extract_ical_property_with_params`.
     * @return Result containing the parsed DateTime or an error
     */
    fn parse_ical_datetime(
        value: &str,
        is_date_only: bool,
    ) -> std::result::Result<DateTime<Utc>, String> {
        // All-day form — YYYYMMDD, 8 chars, no time component. Caller
        // signalled this via the `VALUE=DATE` parameter on the source
        // property. Pre-2026-07-14 this was detected by scanning the
        // raw property line for the substring `VALUE=DATE`, which
        // failed because `extract_ical_property` refused to return
        // param-carrying lines at all (see #528).
        if is_date_only {
            if value.len() != 8 {
                return Err(format!(
                    "Invalid all-day date format: expected YYYYMMDD (8 chars), got {} chars",
                    value.len()
                ));
            }

            let year = value[0..4]
                .parse::<i32>()
                .map_err(|_| "Invalid year".to_string())?;
            let month = value[4..6]
                .parse::<u32>()
                .map_err(|_| "Invalid month".to_string())?;
            let day = value[6..8]
                .parse::<u32>()
                .map_err(|_| "Invalid day".to_string())?;

            return match chrono::NaiveDate::from_ymd_opt(year, month, day) {
                Some(date) => Ok(Utc.from_utc_datetime(&date.and_hms_opt(0, 0, 0).unwrap())),
                None => Err("Invalid date components".to_string()),
            };
        }

        // Standard UTC form: YYYYMMDDTHHMMSSZ, 16 chars, trailing 'Z'.
        // Floating-time (no 'Z') and TZID-anchored forms aren't yet
        // supported — future work when we tackle VTIMEZONE properly.
        if value.len() < 15 || !value.ends_with('Z') {
            return Err(format!(
                "Invalid datetime format: expected YYYYMMDDTHHMMSSZ, got {:?}",
                value
            ));
        }

        let year = value[0..4]
            .parse::<i32>()
            .map_err(|_| "Invalid year".to_string())?;
        let month = value[4..6]
            .parse::<u32>()
            .map_err(|_| "Invalid month".to_string())?;
        let day = value[6..8]
            .parse::<u32>()
            .map_err(|_| "Invalid day".to_string())?;

        let hour = value[9..11]
            .parse::<u32>()
            .map_err(|_| "Invalid hour".to_string())?;
        let minute = value[11..13]
            .parse::<u32>()
            .map_err(|_| "Invalid minute".to_string())?;
        let second = value[13..15]
            .parse::<u32>()
            .map_err(|_| "Invalid second".to_string())?;

        match chrono::NaiveDate::from_ymd_opt(year, month, day) {
            Some(date) => match date.and_hms_opt(hour, minute, second) {
                Some(datetime) => Ok(Utc.from_utc_datetime(&datetime)),
                None => Err("Invalid time components".to_string()),
            },
            None => Err("Invalid date components".to_string()),
        }
    }

    /**
     * Updates an iCalendar property in the event's iCalendar data.
     *
     * @param property_name The name of the property to update
     * @param value The new value for the property
     */
    fn update_ical_property(&mut self, property_name: &str, value: &str) {
        let search_str = format!("\n{}:", property_name);
        let search_str_alt = format!("\r\n{}:", property_name);

        // Check if property exists
        let pos = self
            .ical_data
            .find(&search_str)
            .or_else(|| self.ical_data.find(&search_str_alt));

        if let Some(pos) = pos {
            // Find the start of the value
            let value_start = pos + search_str.len();

            // Find the end of the value (next line or end of string)
            let value_end = self.ical_data[value_start..]
                .find('\n')
                .map(|p| value_start + p)
                .unwrap_or_else(|| self.ical_data.len());

            // Replace the value
            let before = &self.ical_data[..value_start];
            let after = &self.ical_data[value_end..];
            self.ical_data = format!("{}{}{}", before, value, after);
        } else {
            // Property doesn't exist, add it before END:VEVENT
            let end_pos = self
                .ical_data
                .find("END:VEVENT")
                .unwrap_or(self.ical_data.len());

            let before = &self.ical_data[..end_pos];
            let after = &self.ical_data[end_pos..];
            self.ical_data = format!("{}{}:{}\n{}", before, property_name, value, after);
        }
    }

    /**
     * Removes an iCalendar property from the event's iCalendar data.
     *
     * @param property_name The name of the property to remove
     */
    fn remove_ical_property(&mut self, property_name: &str) {
        let search_str = format!("\n{}:", property_name);
        let search_str_alt = format!("\r\n{}:", property_name);

        // Check if property exists
        let pos = self
            .ical_data
            .find(&search_str)
            .or_else(|| self.ical_data.find(&search_str_alt));

        if let Some(pos) = pos {
            // Find the end of the value (next line or end of string)
            let value_end = self.ical_data[pos + 1..]
                .find('\n')
                .map(|p| pos + 1 + p)
                .unwrap_or_else(|| self.ical_data.len());

            // Remove the property
            let before = &self.ical_data[..pos];
            let after = &self.ical_data[value_end..];
            self.ical_data = format!("{}{}", before, after);
        }
    }
}

#[cfg(test)]
mod ical_parser_tests {
    //! Regression tests for the `ical`-crate-backed property extractor.
    //!
    //! Every shape here failed under the pre-2026-07-14 hand-rolled
    //! `find("\n<NAME>:")` scan (see AtalayaLabs/OxiCloud#528). Fixtures
    //! are RFC 5545-shaped; when we bundle real client bodies from
    //! Thunderbird / DAVx⁵ / Gnome Calendar the mapping will follow the
    //! same style — each case declares which shape it exercises.
    //!
    //! Fixture sources / attributions:
    //!   * RFC 5545 §3.6.1 (VEVENT baseline) — timed event example
    //!   * RFC 5545 §3.8.2.4 (DTSTART DATE form) — all-day event
    //!   * RFC 5545 §3.8.4.4 (RECURRENCE-ID) — exception instance
    //!   * Shape adapted from Radicale test fixtures — RRULE + UNTIL
    //!     with a DATE-form UNTIL for an all-day recurring event
    //!
    //! Everything is spec-shaped and byte-small; no network / no
    //! external files. Real client bodies can be added later under
    //! `tests/fixtures/ical/` and loaded via `include_str!`.

    use super::*;

    /// Simple timed VEVENT. Baseline sanity — this shape worked pre-
    /// rewrite (no property parameters), so it's the regression floor.
    const TIMED_EVENT: &str = "\
BEGIN:VCALENDAR\r
VERSION:2.0\r
PRODID:-//OxiCloud test//EN\r
BEGIN:VEVENT\r
UID:timed-1@oxicloud.test\r
DTSTAMP:20260101T100000Z\r
DTSTART:20260101T120000Z\r
DTEND:20260101T130000Z\r
SUMMARY:Timed baseline\r
END:VEVENT\r
END:VCALENDAR\r
";

    /// All-day VEVENT — the exact shape #528 flagged. Property line
    /// carries `;VALUE=DATE:` which the old scan refused; the crate-
    /// backed extractor now parses it and the all-day flag is derived
    /// from the `VALUE` parameter.
    const ALL_DAY_EVENT: &str = "\
BEGIN:VCALENDAR\r
VERSION:2.0\r
PRODID:-//OxiCloud test//EN\r
BEGIN:VEVENT\r
UID:allday-1@oxicloud.test\r
DTSTAMP:20260101T100000Z\r
DTSTART;VALUE=DATE:20260201\r
DTEND;VALUE=DATE:20260202\r
SUMMARY:All-day event\r
END:VEVENT\r
END:VCALENDAR\r
";

    /// Timed recurring master with a modified single occurrence
    /// (RECURRENCE-ID identifies which instance). The exception VEVENT
    /// shares the master's UID and adds `RECURRENCE-ID:` to pinpoint
    /// the overridden date. This is the #528 shape — parser must not
    /// choke on the presence of RECURRENCE-ID even though we don't
    /// route it into the domain yet (that's phase 2).
    const RECURRING_WITH_EXCEPTION: &str = "\
BEGIN:VCALENDAR\r
VERSION:2.0\r
PRODID:-//OxiCloud test//EN\r
BEGIN:VEVENT\r
UID:daily-1@oxicloud.test\r
DTSTAMP:20260101T100000Z\r
DTSTART:20260101T090000Z\r
DTEND:20260101T100000Z\r
SUMMARY:Daily standup\r
RRULE:FREQ=DAILY;COUNT=10\r
END:VEVENT\r
BEGIN:VEVENT\r
UID:daily-1@oxicloud.test\r
DTSTAMP:20260101T100000Z\r
DTSTART:20260103T110000Z\r
DTEND:20260103T120000Z\r
SUMMARY:Daily standup — rescheduled\r
RECURRENCE-ID:20260103T090000Z\r
END:VEVENT\r
END:VCALENDAR\r
";

    /// All-day recurring with an all-day exception — the most-broken
    /// case in #528 (RECURRENCE-ID;VALUE=DATE:...). Parser must accept
    /// the parameter on both DTSTART and RECURRENCE-ID.
    const ALL_DAY_RECURRING_WITH_EXCEPTION: &str = "\
BEGIN:VCALENDAR\r
VERSION:2.0\r
PRODID:-//OxiCloud test//EN\r
BEGIN:VEVENT\r
UID:weekly-allday@oxicloud.test\r
DTSTAMP:20260101T100000Z\r
DTSTART;VALUE=DATE:20260105\r
DTEND;VALUE=DATE:20260106\r
SUMMARY:Weekly all-day\r
RRULE:FREQ=WEEKLY;COUNT=4\r
END:VEVENT\r
BEGIN:VEVENT\r
UID:weekly-allday@oxicloud.test\r
DTSTAMP:20260101T100000Z\r
DTSTART;VALUE=DATE:20260113\r
DTEND;VALUE=DATE:20260114\r
SUMMARY:Weekly all-day — rescheduled\r
RECURRENCE-ID;VALUE=DATE:20260112\r
END:VEVENT\r
END:VCALENDAR\r
";

    fn parse_ok(body: &str) -> CalendarEvent {
        CalendarEvent::from_ical(Uuid::new_v4(), body.to_string())
            .expect("expected successful parse")
    }

    #[test]
    fn timed_event_parses_and_is_not_all_day() {
        let ev = parse_ok(TIMED_EVENT);
        assert_eq!(ev.summary(), "Timed baseline");
        assert!(!ev.all_day());
    }

    #[test]
    fn all_day_event_parses_and_flags_as_all_day() {
        // Regression: DTSTART;VALUE=DATE:20260201 used to fail
        // property-extraction ("Missing DTSTART") because the raw
        // scan required a colon directly after the property name.
        let ev = parse_ok(ALL_DAY_EVENT);
        assert!(ev.all_day(), "VALUE=DATE parameter should flag all-day");
        assert_eq!(
            ev.start_time().date_naive().to_string(),
            "2026-02-01",
            "DTSTART value should parse the YYYYMMDD payload"
        );
    }

    #[test]
    fn recurring_with_exception_still_returns_the_master() {
        // The crate parses BOTH events from the VCALENDAR body; our
        // `parse_first_vevent` returns the first, which is the master.
        // Exception routing is phase 2 — this test locks the current
        // "first event wins" behavior so phase 2 knows what it's
        // extending.
        let ev = parse_ok(RECURRING_WITH_EXCEPTION);
        assert_eq!(ev.summary(), "Daily standup");
        assert_eq!(ev.ical_uid(), "daily-1@oxicloud.test");
        assert_eq!(ev.rrule().as_deref(), Some("FREQ=DAILY;COUNT=10"));
    }

    #[test]
    fn all_day_recurring_with_exception_master_parses() {
        // The #528 shape end-to-end: parameterised DTSTART on both the
        // master and the exception, plus a parameterised RECURRENCE-ID.
        // Pre-rewrite this was a 400 (post the error-mapping fix) or 500
        // (before it); post-rewrite the master parses cleanly and the
        // all_day flag is set from the master's DTSTART parameters.
        let ev = parse_ok(ALL_DAY_RECURRING_WITH_EXCEPTION);
        assert!(ev.all_day());
        assert_eq!(ev.ical_uid(), "weekly-allday@oxicloud.test");
    }

    #[test]
    fn missing_dtstart_still_returns_a_useful_error() {
        // Preserve the pre-rewrite error contract for the genuinely-
        // missing case. `dav_error_mapping.hurl` asserts this shape.
        let body = "\
BEGIN:VCALENDAR\r
VERSION:2.0\r
PRODID:-//OxiCloud test//EN\r
BEGIN:VEVENT\r
UID:missing-dtstart@oxicloud.test\r
DTSTAMP:20260101T100000Z\r
DTEND:20260101T130000Z\r
SUMMARY:No DTSTART\r
END:VEVENT\r
END:VCALENDAR\r
";
        let err = CalendarEvent::from_ical(Uuid::new_v4(), body.to_string())
            .expect_err("expected InvalidInput for missing DTSTART");
        assert_eq!(err.kind, ErrorKind::InvalidInput);
        assert!(
            err.message.contains("DTSTART"),
            "message should mention DTSTART, got: {}",
            err.message
        );
    }

    #[test]
    fn extract_property_with_params_returns_parameter_map() {
        // Direct test of the params-aware extractor. Confirms
        // parameter names are normalised to uppercase and preserved
        // as a list (RFC 5545 §3.2 — parameters can carry multiple
        // comma-separated values).
        let (value, params) =
            CalendarEvent::extract_ical_property_with_params(ALL_DAY_EVENT, "DTSTART")
                .expect("DTSTART must extract");
        assert_eq!(value, "20260201");
        let vals = params.get("VALUE").expect("VALUE param must be present");
        assert_eq!(vals, &vec!["DATE".to_string()]);
    }

    #[test]
    fn extract_property_case_insensitive_property_name() {
        // Property names are ASCII case-insensitive per RFC 5545 §3.1.
        // The lookup must accept "dtstart" as well as "DTSTART".
        let v = CalendarEvent::extract_ical_property(TIMED_EVENT, "dtstart");
        assert_eq!(v.as_deref(), Some("20260101T120000Z"));
    }
}
