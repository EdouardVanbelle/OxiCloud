# =============================================================
# OxiCloud — CalDAV calendar-query REPORT time-range regression
# =============================================================
# Regression pin for the time-range parser fix on
# fix/caldav-time-range-parser.
#
# Pre-fix: caldav_adapter.rs::parse_report used
# `DateTime::parse_from_rfc3339` on the `<C:time-range start="..."
# end="..."/>` attribute values. That parser expects
# `YYYY-MM-DDTHH:MM:SSZ` (dashes + colons). CalDAV clients send
# iCalendar DATE-TIME format (`YYYYMMDDTHHMMSSZ` — no separators)
# per RFC 4791 §9.9 / RFC 5545 §3.3.5. Result: parse silently
# failed, `time_range` was `None`, and the REPORT handler fell
# through to `list_events`, returning the ENTIRE calendar
# regardless of the requested window.
#
# Post-fix: `parse_caldav_datetime` accepts iCal DATE-TIME
# (the standard) with RFC 3339 as a defensive fallback. Time-
# range filters now actually filter.
#
# Test shape: create two events at 09:00 UTC and 15:00 UTC, then
# calendar-query REPORT with an iCal-DATE-TIME window covering
# only the 09:00 event. Assert the response contains the 09:00
# event's UID and does NOT contain the 15:00 event's UID.
# =============================================================


# ─────────────────────────────────────────────────────────────
# Step 1 — Admin login.
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/auth/login
Content-Type: application/json
{ "username": "{{username}}", "password": "{{password}}" }

HTTP 200
[Captures]
admin_token: jsonpath "$.access_token"


# ─────────────────────────────────────────────────────────────
# Step 2 — Provision a fresh calendar (`tr-cal`) so the seeded
# events don't collide with anything downstream tests provisioned.
# MKCALENDAR returns 201; the server assigns its own UUID which
# we capture via PROPFIND in Step 3.
# ─────────────────────────────────────────────────────────────
MKCALENDAR {{base_url}}/caldav/tr-cal/
Authorization: Bearer {{admin_token}}

HTTP 201


# ─────────────────────────────────────────────────────────────
# Step 3 — Discover the server-assigned UUID via PROPFIND. The
# `(?s).*` anchor greedy-matches to the LAST /caldav/<uuid>/ in
# the body — that's `tr-cal`, freshest by created_at.
# ─────────────────────────────────────────────────────────────
PROPFIND {{base_url}}/caldav/
Authorization: Bearer {{admin_token}}
Depth: 1
Content-Type: application/xml
```
<?xml version="1.0" encoding="UTF-8"?>
<D:propfind xmlns:D="DAV:">
  <D:prop><D:displayname/><D:resourcetype/></D:prop>
</D:propfind>
```

HTTP 207
[Captures]
tr_cal_id: body regex "(?s).*/caldav/([a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12})/"
[Asserts]
body contains "tr-cal"


# ─────────────────────────────────────────────────────────────
# Step 4 — Seed the morning event (09:00–10:00 UTC).
# ─────────────────────────────────────────────────────────────
PUT {{base_url}}/caldav/{{tr_cal_id}}/tr-morning.ics
Authorization: Bearer {{admin_token}}
Content-Type: text/calendar
```
BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//OxiCloud time-range test//EN
BEGIN:VEVENT
UID:tr-morning
DTSTAMP:20260101T080000Z
DTSTART:20260101T090000Z
DTEND:20260101T100000Z
SUMMARY:Morning event
END:VEVENT
END:VCALENDAR
```

HTTP *
[Asserts]
status >= 200
status < 300


# ─────────────────────────────────────────────────────────────
# Step 5 — Seed the afternoon event (15:00–16:00 UTC).
# ─────────────────────────────────────────────────────────────
PUT {{base_url}}/caldav/{{tr_cal_id}}/tr-afternoon.ics
Authorization: Bearer {{admin_token}}
Content-Type: text/calendar
```
BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//OxiCloud time-range test//EN
BEGIN:VEVENT
UID:tr-afternoon
DTSTAMP:20260101T080000Z
DTSTART:20260101T150000Z
DTEND:20260101T160000Z
SUMMARY:Afternoon event
END:VEVENT
END:VCALENDAR
```

HTTP *
[Asserts]
status >= 200
status < 300


# ─────────────────────────────────────────────────────────────
# Step 6 — calendar-query REPORT with iCal-DATE-TIME time-range
# covering the morning event only (08:00 → 13:00 UTC). Post-fix
# the response must contain `tr-morning` and MUST NOT contain
# `tr-afternoon`.
#
# Pre-fix: `time_range` parses as None → falls through to
# `list_events`, response contains BOTH events. This test's
# "body not contains tr-afternoon" assertion catches that.
# ─────────────────────────────────────────────────────────────
REPORT {{base_url}}/caldav/{{tr_cal_id}}/
Authorization: Bearer {{admin_token}}
Content-Type: application/xml
Depth: 1
```
<?xml version="1.0" encoding="UTF-8"?>
<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop>
    <D:getetag/>
    <C:calendar-data/>
  </D:prop>
  <C:filter>
    <C:comp-filter name="VCALENDAR">
      <C:comp-filter name="VEVENT">
        <C:time-range start="20260101T080000Z" end="20260101T130000Z"/>
      </C:comp-filter>
    </C:comp-filter>
  </C:filter>
</C:calendar-query>
```

HTTP 207
[Asserts]
body contains "tr-morning"
body not contains "tr-afternoon"


# ─────────────────────────────────────────────────────────────
# Step 7 — Symmetric window: afternoon only (14:00 → 17:00 UTC).
# Guards against a fix that accidentally hardcodes the morning
# window or reverses start/end.
# ─────────────────────────────────────────────────────────────
REPORT {{base_url}}/caldav/{{tr_cal_id}}/
Authorization: Bearer {{admin_token}}
Content-Type: application/xml
Depth: 1
```
<?xml version="1.0" encoding="UTF-8"?>
<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop>
    <D:getetag/>
    <C:calendar-data/>
  </D:prop>
  <C:filter>
    <C:comp-filter name="VCALENDAR">
      <C:comp-filter name="VEVENT">
        <C:time-range start="20260101T140000Z" end="20260101T170000Z"/>
      </C:comp-filter>
    </C:comp-filter>
  </C:filter>
</C:calendar-query>
```

HTTP 207
[Asserts]
body contains "tr-afternoon"
body not contains "tr-morning"


# ─────────────────────────────────────────────────────────────
# Step 8 — Window with no overlap (year 2027) returns neither
# event. Proves the filter is actually applied (pre-fix this
# returned both).
# ─────────────────────────────────────────────────────────────
REPORT {{base_url}}/caldav/{{tr_cal_id}}/
Authorization: Bearer {{admin_token}}
Content-Type: application/xml
Depth: 1
```
<?xml version="1.0" encoding="UTF-8"?>
<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop>
    <D:getetag/>
    <C:calendar-data/>
  </D:prop>
  <C:filter>
    <C:comp-filter name="VCALENDAR">
      <C:comp-filter name="VEVENT">
        <C:time-range start="20270101T000000Z" end="20270102T000000Z"/>
      </C:comp-filter>
    </C:comp-filter>
  </C:filter>
</C:calendar-query>
```

HTTP 207
[Asserts]
body not contains "tr-morning"
body not contains "tr-afternoon"


# ─────────────────────────────────────────────────────────────
# Step 9 — Sanity: a filter-less REPORT still returns both
# events. Guards against a fix that over-corrects and starts
# treating "no time-range" as "empty window".
# ─────────────────────────────────────────────────────────────
REPORT {{base_url}}/caldav/{{tr_cal_id}}/
Authorization: Bearer {{admin_token}}
Content-Type: application/xml
Depth: 1
```
<?xml version="1.0" encoding="UTF-8"?>
<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop>
    <D:getetag/>
    <C:calendar-data/>
  </D:prop>
</C:calendar-query>
```

HTTP 207
[Asserts]
body contains "tr-morning"
body contains "tr-afternoon"


# ─────────────────────────────────────────────────────────────
# Step 10 — Cleanup: delete the calendar so downstream test
# files don't inherit an extra collection (per memory
# feedback_hurl_teardown_shared_db).
# ─────────────────────────────────────────────────────────────
DELETE {{base_url}}/caldav/{{tr_cal_id}}/
Authorization: Bearer {{admin_token}}

HTTP 204
