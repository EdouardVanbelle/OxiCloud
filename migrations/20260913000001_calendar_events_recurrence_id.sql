-- ════════════════════════════════════════════════════════════════════════════
-- caldav.calendar_events — add RECURRENCE-ID column for exception instances
-- ════════════════════════════════════════════════════════════════════════════
-- Motivation: AtalayaLabs/OxiCloud#528 — CalDAV clients (Thunderbird, Apple
-- Calendar, Gnome Calendar, DAVx⁵) modify a single occurrence of a recurring
-- event by PUTting a separate VEVENT that shares the master's UID and adds
-- a RECURRENCE-ID identifying which occurrence is overridden (RFC 5545
-- §3.8.4.4).
--
-- Pre-#528 behaviour: modifications either hit a UID collision (silent
-- 500 or corrupt state) or overwrote the master. Post-#528 the exception
-- override lives as its own row keyed by
-- (calendar_id, ical_uid, recurrence_id), with the master identified by
-- `recurrence_id IS NULL`.
--
-- Related but distinct from parser Phase 1 (rewrite of extract_ical_property
-- on top of the `ical` crate) — that landed in the same branch to enable
-- parsing RECURRENCE-ID at all. This migration is the storage half.
--
-- No backfill needed — pre-migration events all become masters (NULL). No
-- existing exception rows existed because the parser couldn't read them.
-- ════════════════════════════════════════════════════════════════════════════

BEGIN;

-- Column: nullable. NULL = master, non-NULL = exception instance whose
-- value pinpoints which occurrence of the recurring master is being
-- overridden. TIMESTAMPTZ so both timed (DATE-TIME) and all-day (DATE)
-- RECURRENCE-IDs fit — the domain-side `parse_ical_datetime` normalises
-- both into `DateTime<Utc>` (all-day → midnight UTC of the target date).
ALTER TABLE caldav.calendar_events
    ADD COLUMN recurrence_id TIMESTAMP WITH TIME ZONE NULL;

COMMENT ON COLUMN caldav.calendar_events.recurrence_id IS
    'RFC 5545 §3.8.4.4 RECURRENCE-ID. NULL on the master, non-NULL on '
    'per-instance exception overrides. Keyed with (calendar_id, ical_uid) '
    'via the two partial unique indexes below.';

-- Partial unique index: at most one master row per (calendar_id, ical_uid).
--
-- Without this a client that re-uses a UID across calendar events (e.g. a
-- pre-2026-08 import that didn't dedupe) could produce two masters — the
-- lookup by (calendar_id, ical_uid) WHERE recurrence_id IS NULL would then
-- be ambiguous and the exception-routing logic would either overwrite the
-- wrong master or refuse to insert. Pre-migration duplicates would fail
-- this index creation; if that happens, the reconciliation is out of scope
-- for this migration (dedup script would go here — but the existing
-- codebase generates fresh UIDs on ambiguity so it shouldn't fire in
-- practice).
CREATE UNIQUE INDEX idx_calendar_events_master_unique
    ON caldav.calendar_events (calendar_id, ical_uid)
    WHERE recurrence_id IS NULL;

-- Partial unique index: at most one exception override per
-- (calendar_id, ical_uid, recurrence_id). Prevents two rows both claiming
-- to override the same instance of the same master — which would confuse
-- the client on next PROPFIND.
CREATE UNIQUE INDEX idx_calendar_events_exception_unique
    ON caldav.calendar_events (calendar_id, ical_uid, recurrence_id)
    WHERE recurrence_id IS NOT NULL;

-- Read-path index for the "give me the master + all its exceptions"
-- query the PROPFIND handler will run. Covered by the two unique indexes
-- above only partially — this covering index reads the full
-- (calendar_id, ical_uid) pair in one seek regardless of which side of
-- the master/exception split.
CREATE INDEX idx_calendar_events_uid_lookup
    ON caldav.calendar_events (calendar_id, ical_uid);

COMMIT;
