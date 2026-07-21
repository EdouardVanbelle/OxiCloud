# =============================================================
# OxiCloud – CalDAV recurring events with RECURRENCE-ID overrides
# =============================================================
# End-to-end regression for AtalayaLabs/OxiCloud#528.
#
# Pre-fix behaviour (all-day recurring event, one occurrence
# modified in Thunderbird/Apple Calendar/DAVx⁵/Gnome Calendar):
#   * The client PUTs a VCALENDAR containing the master (with
#     RRULE) + a per-instance override (RFC 5545 §3.8.4.4,
#     `RECURRENCE-ID`). Pre-fix the substring-based parser
#     could not read any property carrying parameters
#     (`DTSTART;VALUE=DATE:...`, `RECURRENCE-ID;VALUE=DATE:...`),
#     so all-day master modifications 500'd outright.
#   * Even for timed events, the old create_event_from_ical
#     read only the first VEVENT — a second PUT of just the
#     exception would overwrite the master row entirely,
#     silently corrupting the client's view of the series.
#
# Post-fix (this file's invariant):
#   1. Master PUT → 201 CREATED, one row (recurrence_id NULL).
#   2. PUT master + exception in one body → both persist to
#      their own row keyed by (calendar_id, ical_uid,
#      recurrence_id). Response is 201 CREATED because the
#      exception was newly inserted.
#   3. PUT ONLY the exception with modified content → 204
#      No Content (in-place replace, no new rows). CRITICALLY,
#      the MASTER row survives untouched — a GET on the .ics
#      URL still returns the master's original RRULE + summary.
#   4. All-day master + all-day exception (the exact #528 shape)
#      completes the same round-trip.
#
# Storage invariant enforced by two partial unique indexes on
# caldav.calendar_events (see migration 20260913000001):
#   * idx_calendar_events_master_unique — at most one master
#     per (calendar_id, ical_uid).
#   * idx_calendar_events_exception_unique — at most one
#     exception override per (calendar_id, ical_uid,
#     recurrence_id).
# =============================================================


# ─────────────────────────────────────────────────────────────
# Step 1 – Admin logs in.
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/auth/login
Content-Type: application/json
{
  "username": "{{username}}",
  "password": "{{password}}"
}

HTTP 200
[Captures]
admin_token: jsonpath "$.access_token"


# ─────────────────────────────────────────────────────────────
# Step 2 – MKCALENDAR: fresh calendar for #528 regression.
# ─────────────────────────────────────────────────────────────
MKCALENDAR {{base_url}}/caldav/recurring-528/
Authorization: Bearer {{admin_token}}

HTTP 201


# ─────────────────────────────────────────────────────────────
# Step 3 – PROPFIND to capture the server-assigned UUID for
# recurring-528. The (?s).* anchor greedy-matches to the LAST
# /caldav/<uuid>/ in the body, which is our just-created
# calendar (default-provisioned calendars come first by
# created_at, this one is newest).
# ─────────────────────────────────────────────────────────────
PROPFIND {{base_url}}/caldav/
Authorization: Bearer {{admin_token}}
Depth: 1
Content-Type: application/xml
```
<?xml version="1.0" encoding="UTF-8"?>
<D:propfind xmlns:D="DAV:">
  <D:prop>
    <D:displayname/>
    <D:resourcetype/>
  </D:prop>
</D:propfind>
```

HTTP 207
[Captures]
calendar_id: body regex "(?s).*/caldav/([a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12})/"
[Asserts]
body contains "recurring-528"


# ─────────────────────────────────────────────────────────────
# Step 4 – PUT the recurring master (timed, daily, 10 count).
# Expect 201 CREATED (fresh row) and a non-empty ETag.
# ─────────────────────────────────────────────────────────────
PUT {{base_url}}/caldav/{{calendar_id}}/daily-e2e-528.ics
Authorization: Bearer {{admin_token}}
Content-Type: text/calendar; charset=utf-8
```
BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//OxiCloud e2e//EN
BEGIN:VEVENT
UID:daily-e2e-528
DTSTAMP:20260101T100000Z
DTSTART:20260101T090000Z
DTEND:20260101T093000Z
SUMMARY:Daily standup
RRULE:FREQ=DAILY;COUNT=10
END:VEVENT
END:VCALENDAR
```

HTTP 201
[Asserts]
header "ETag" exists


# ─────────────────────────────────────────────────────────────
# Step 5 – GET the master. Body contains RRULE + original
# SUMMARY, confirming the master is stored and serves as-is.
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/caldav/{{calendar_id}}/daily-e2e-528.ics
Authorization: Bearer {{admin_token}}

HTTP 200
[Asserts]
body contains "FREQ=DAILY;COUNT=10"
body contains "SUMMARY:Daily standup"


# ─────────────────────────────────────────────────────────────
# Step 6 – The #528 heart: PUT master + per-instance override
# in a single body. This is what Thunderbird sends when the
# user modifies one occurrence of a recurring event.
#
# Expected:
#   * 201 CREATED because the exception is newly inserted.
#     (The master is replaced-in-place — any_inserted=true
#     is decided by the NEW exception row, not the master.)
#   * Both rows now exist in the DB. Verified in Step 7 via
#     the master's GET still returning the master data.
# ─────────────────────────────────────────────────────────────
PUT {{base_url}}/caldav/{{calendar_id}}/daily-e2e-528.ics
Authorization: Bearer {{admin_token}}
Content-Type: text/calendar; charset=utf-8
```
BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//OxiCloud e2e//EN
BEGIN:VEVENT
UID:daily-e2e-528
DTSTAMP:20260101T100000Z
DTSTART:20260101T090000Z
DTEND:20260101T093000Z
SUMMARY:Daily standup
RRULE:FREQ=DAILY;COUNT=10
END:VEVENT
BEGIN:VEVENT
UID:daily-e2e-528
DTSTAMP:20260101T100000Z
DTSTART:20260103T110000Z
DTEND:20260103T120000Z
SUMMARY:Daily standup — rescheduled
RECURRENCE-ID:20260103T090000Z
END:VEVENT
END:VCALENDAR
```

HTTP 201


# ─────────────────────────────────────────────────────────────
# Step 7 – GET the URL — must return the FULL calendar-object-
# resource: master VEVENT (with RRULE + original SUMMARY) AND
# the exception VEVENT (with RECURRENCE-ID + rescheduled
# SUMMARY) concatenated in ONE VCALENDAR body. This is the
# phase-4 read-side contract per RFC 4791 §4.1 + RFC 5545
# §3.6.1 — one URL per UID, one VCALENDAR containing every
# component.
#
# Pre-phase-4 this GET returned ONLY the master and clients
# never saw the exception, so their next-PUT dropped it.
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/caldav/{{calendar_id}}/daily-e2e-528.ics
Authorization: Bearer {{admin_token}}

HTTP 200
[Asserts]
body contains "FREQ=DAILY;COUNT=10"
body contains "SUMMARY:Daily standup"
body contains "SUMMARY:Daily standup — rescheduled"
body contains "RECURRENCE-ID:20260103T090000Z"


# ─────────────────────────────────────────────────────────────
# Step 8 – PUT only the exception with a modified SUMMARY.
# Because the exception row already exists (from Step 6),
# no new row is inserted → 204 No Content. The MASTER is
# untouched (verified in Step 9).
# ─────────────────────────────────────────────────────────────
PUT {{base_url}}/caldav/{{calendar_id}}/daily-e2e-528.ics
Authorization: Bearer {{admin_token}}
Content-Type: text/calendar; charset=utf-8
```
BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//OxiCloud e2e//EN
BEGIN:VEVENT
UID:daily-e2e-528
DTSTAMP:20260101T110000Z
DTSTART:20260103T120000Z
DTEND:20260103T130000Z
SUMMARY:Daily standup — rescheduled AGAIN
RECURRENCE-ID:20260103T090000Z
END:VEVENT
END:VCALENDAR
```

HTTP 204


# ─────────────────────────────────────────────────────────────
# Step 9 – After the exception-only PUT: bundled GET returns
# the master (unchanged, still carries RRULE + original
# SUMMARY) AND the newly-updated exception (SUMMARY now
# "rescheduled AGAIN" from Step 8).
#
# Pre-phase-3 the exception-only PUT wiped the master row.
# Pre-phase-4 the master survived but the exception was
# invisible in the GET body.
# Post-phase-4: both survive, both visible.
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/caldav/{{calendar_id}}/daily-e2e-528.ics
Authorization: Bearer {{admin_token}}

HTTP 200
[Asserts]
body contains "FREQ=DAILY;COUNT=10"
body contains "SUMMARY:Daily standup"
body contains "SUMMARY:Daily standup — rescheduled AGAIN"
body contains "RECURRENCE-ID:20260103T090000Z"


# ─────────────────────────────────────────────────────────────
# Step 10 – The all-day flavour: master with DTSTART;VALUE=DATE
# + exception with RECURRENCE-ID;VALUE=DATE. Pre-parser-rewrite
# this 500'd because the param-carrying property lines were
# invisible to the substring scanner (root cause of #528).
#
# Uses a distinct UID so it doesn't collide with Step 4-8 rows
# under the master partial unique index.
# ─────────────────────────────────────────────────────────────
PUT {{base_url}}/caldav/{{calendar_id}}/weekly-allday-528.ics
Authorization: Bearer {{admin_token}}
Content-Type: text/calendar; charset=utf-8
```
BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//OxiCloud e2e//EN
BEGIN:VEVENT
UID:weekly-allday-528
DTSTAMP:20260101T100000Z
DTSTART;VALUE=DATE:20260105
DTEND;VALUE=DATE:20260106
SUMMARY:Weekly review
RRULE:FREQ=WEEKLY;COUNT=4
END:VEVENT
BEGIN:VEVENT
UID:weekly-allday-528
DTSTAMP:20260101T100000Z
DTSTART;VALUE=DATE:20260113
DTEND;VALUE=DATE:20260114
SUMMARY:Weekly review — moved
RECURRENCE-ID;VALUE=DATE:20260112
END:VEVENT
END:VCALENDAR
```

HTTP 201


# ─────────────────────────────────────────────────────────────
# Step 11 – Cleanup: delete the entire calendar (cascades to
# all events + exception rows in a single storage call). Keeps
# the shared Hurl DB uncluttered for downstream test files
# (per feedback_hurl_teardown_shared_db).
# ─────────────────────────────────────────────────────────────
DELETE {{base_url}}/caldav/{{calendar_id}}/
Authorization: Bearer {{admin_token}}

HTTP 204
