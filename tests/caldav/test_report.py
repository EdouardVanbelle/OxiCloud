"""CalDAV REPORT method coverage via python-caldav.

The REPORT verb (RFC 4791 §7) is how clients do bulk sync + filtered
lookup. Three subtypes matter for OxiCloud's server surface:

  * `calendar-query` (§7.8) — filter events by time-range /
    property. `search(start=..., end=...)` in pycaldav emits this.
  * `calendar-multiget` (§7.9) — batch fetch by href list. Used
    when the client already knows which UIDs it wants.
  * `sync-collection` (§7.9 / RFC 6578) — token-based delta sync.
    Not exercised here yet — the server delegates to `list_events`
    (no per-token filtering), so a coverage test would just
    replicate the calendar-query case. Leave for later once real
    sync-token support lands.

The tests seed a fresh calendar with three timed events an hour
apart, then exercise each REPORT shape. Row-count assertions are
safe here because the seeded events are all masters (non-recurring),
so master/exception folding doesn't apply — one URL per UID matches
one row in DB.
"""

from __future__ import annotations

import textwrap
import uuid
from datetime import datetime, timezone

import caldav
import pytest


_TIME_RANGE_PARSER_BUG_REASON = (
    "caldav_adapter.rs:~105 + ~172 parses time-range start/end as "
    "RFC 3339 (`2026-01-01T09:30:00Z`), but CalDAV clients send "
    "iCalendar DATE-TIME (`20260101T093000Z` — RFC 4791 §9.9). "
    "Parse fails, time_range becomes None, handle_report falls "
    "through to list_events → returns every event regardless of "
    "window. Fix: chrono::NaiveDateTime::parse_from_str with "
    "`%Y%m%dT%H%M%SZ` (RFC 3339 as fallback). Own fix branch, "
    "e.g. fix/caldav-time-range-parser."
)


# ─────────────────────────────────────────────────────────────
# Helpers — mirror the pattern from test_recurring.py /
# test_ical_coverage.py. Deliberately duplicated for now;
# promote to conftest.py once a fourth test file shows up.
# ─────────────────────────────────────────────────────────────


def _dedent(ical: str) -> str:
    return textwrap.dedent(ical).strip().replace("\n", "\r\n") + "\r\n"


def _put_ical(calendar: caldav.Calendar, uid: str, body: str) -> None:
    url = str(calendar.url).rstrip("/") + f"/{uid}.ics"
    r = calendar.client.request(
        url,
        method="PUT",
        body=body,
        headers={"Content-Type": "text/calendar; charset=utf-8"},
    )
    if r.status < 200 or r.status >= 300:
        raise AssertionError(
            f"PUT {url} → HTTP {r.status}\nbody: {body!r}\nresponse: {r.raw!r}"
        )


def _seed_three_events(calendar: caldav.Calendar) -> list[str]:
    """Seed three non-recurring events, one hour apart, starting
    2026-01-01T09:00 UTC. Returns the list of UIDs in wall-clock
    order (index 0 = earliest).

    Non-recurring is deliberate: it isolates REPORT semantics from
    master/exception folding (which is phase-4 territory)."""
    uids: list[str] = []
    times = [
        ("20260101T090000Z", "20260101T093000Z", "Morning standup"),
        ("20260101T100000Z", "20260101T110000Z", "Mid-morning sync"),
        ("20260101T140000Z", "20260101T150000Z", "Afternoon review"),
    ]
    for start, end, summary in times:
        uid = f"report-{uuid.uuid4().hex[:8]}"
        _put_ical(
            calendar,
            uid,
            _dedent(
                f"""\
                BEGIN:VCALENDAR
                VERSION:2.0
                PRODID:-//pycaldav report coverage//EN
                BEGIN:VEVENT
                UID:{uid}
                DTSTAMP:20260101T080000Z
                DTSTART:{start}
                DTEND:{end}
                SUMMARY:{summary}
                END:VEVENT
                END:VCALENDAR
                """
            ),
        )
        uids.append(uid)
    return uids


# ─────────────────────────────────────────────────────────────
# calendar-query REPORT
# ─────────────────────────────────────────────────────────────


@pytest.mark.xfail(reason=_TIME_RANGE_PARSER_BUG_REASON, strict=False)
def test_calendar_query_time_range_returns_events_in_window(
    fresh_calendar: caldav.Calendar,
) -> None:
    """A time-range filter that spans the middle of the seeded
    day should return only the events whose (DTSTART, DTEND)
    overlaps the window. RFC 4791 §9.9 defines overlap: an event
    overlaps a range if DTSTART < range_end AND DTEND > range_start."""
    uids = _seed_three_events(fresh_calendar)

    # Window: 09:30 → 12:00 UTC. Overlaps events 0 (09:00–09:30
    # touches the boundary at 09:30; RFC excludes exact touch)
    # and event 1 (10:00–11:00, wholly inside). Excludes event 2
    # (14:00–15:00, well outside).
    window_start = datetime(2026, 1, 1, 9, 30, tzinfo=timezone.utc)
    window_end = datetime(2026, 1, 1, 12, 0, tzinfo=timezone.utc)

    found = fresh_calendar.search(
        start=window_start,
        end=window_end,
        event=True,
        expand=False,
    )
    found_uids = {_uid_from_event_data(e.data) for e in found}

    # Event 1 (10:00–11:00) is definitely in-window; event 2 (14:00–
    # 15:00) is definitely out. Event 0's overlap is boundary-
    # dependent (server interpretation varies at exact-touch). The
    # strong invariant: event 1 in, event 2 out.
    assert uids[1] in found_uids, (
        f"Event 1 (mid-morning, wholly inside window) missing from "
        f"time-range REPORT. Got: {found_uids}"
    )
    assert uids[2] not in found_uids, (
        f"Event 2 (afternoon, wholly outside window) leaked into "
        f"time-range REPORT. Got: {found_uids}"
    )


@pytest.mark.xfail(reason=_TIME_RANGE_PARSER_BUG_REASON, strict=False)
def test_calendar_query_time_range_after_all_events_returns_empty(
    fresh_calendar: caldav.Calendar,
) -> None:
    """A window that starts after every seeded event returns
    zero results — proves the range filter is actually applied,
    not silently ignored (which would surface as "all events
    returned regardless of window")."""
    _seed_three_events(fresh_calendar)

    window_start = datetime(2027, 1, 1, 0, 0, tzinfo=timezone.utc)
    window_end = datetime(2027, 1, 2, 0, 0, tzinfo=timezone.utc)

    found = fresh_calendar.search(
        start=window_start,
        end=window_end,
        event=True,
        expand=False,
    )
    assert found == [], (
        f"Expected empty result for window one year past all seeded "
        f"events; got {len(found)} entries."
    )


@pytest.mark.xfail(reason=_TIME_RANGE_PARSER_BUG_REASON, strict=False)
def test_calendar_query_time_range_before_all_events_returns_empty(
    fresh_calendar: caldav.Calendar,
) -> None:
    """Symmetric to the after-window case."""
    _seed_three_events(fresh_calendar)

    window_start = datetime(2025, 1, 1, 0, 0, tzinfo=timezone.utc)
    window_end = datetime(2025, 1, 2, 0, 0, tzinfo=timezone.utc)

    found = fresh_calendar.search(
        start=window_start,
        end=window_end,
        event=True,
        expand=False,
    )
    assert found == []


def test_calendar_query_no_filter_returns_every_event(
    fresh_calendar: caldav.Calendar,
) -> None:
    """`calendar.events()` (pycaldav) issues a calendar-query without
    a time-range — the server routes this via `list_events`, so
    every event in the calendar surfaces. Row count = 3 seeded
    events (all non-recurring, so 1 URL per row)."""
    uids = _seed_three_events(fresh_calendar)

    all_events = fresh_calendar.events()
    found_uids = {_uid_from_event_data(e.data) for e in all_events}

    for expected in uids:
        assert expected in found_uids, (
            f"Seeded event {expected} missing from unfiltered "
            f"calendar-query REPORT. Got: {found_uids}"
        )


# ─────────────────────────────────────────────────────────────
# calendar-multiget REPORT
# ─────────────────────────────────────────────────────────────


def test_calendar_multiget_by_href_returns_the_targeted_events(
    fresh_calendar: caldav.Calendar,
) -> None:
    """calendar-multiget takes an explicit href list and returns
    exactly those. Two hrefs → two responses. The server's
    `get_events_by_ical_uids` (indexed `ical_uid = ANY(...)`) is
    what pays for this instead of listing the whole calendar."""
    uids = _seed_three_events(fresh_calendar)

    base = str(fresh_calendar.url).rstrip("/") + "/"
    # Target the first two events; skip event 2.
    hrefs = [f"{base}{uids[0]}.ics", f"{base}{uids[1]}.ics"]
    xml = _multiget_body(hrefs)

    r = fresh_calendar.client.request(
        str(fresh_calendar.url),
        method="REPORT",
        body=xml,
        headers={"Content-Type": "application/xml; charset=utf-8", "Depth": "1"},
    )
    assert 200 <= r.status < 300, (
        f"REPORT calendar-multiget → HTTP {r.status}\nbody: {r.raw!r}"
    )
    xml_body = r.raw.decode("utf-8") if isinstance(r.raw, bytes) else r.raw

    assert uids[0] in xml_body, (
        f"Requested UID {uids[0]} missing from multiget response."
    )
    assert uids[1] in xml_body, (
        f"Requested UID {uids[1]} missing from multiget response."
    )
    assert uids[2] not in xml_body, (
        f"UID {uids[2]} (not requested) leaked into multiget response."
    )


def test_calendar_multiget_unknown_href_is_silently_absent(
    fresh_calendar: caldav.Calendar,
) -> None:
    """CalDAV multiget semantics: a requested href that doesn't
    exist is silently absent from the response (not an error).
    Some servers emit a `<D:status>404</D:status>` per-href entry;
    the minimum bar is that the server must NOT 500 and must NOT
    invent data."""
    uids = _seed_three_events(fresh_calendar)

    base = str(fresh_calendar.url).rstrip("/") + "/"
    ghost_uid = f"does-not-exist-{uuid.uuid4().hex[:8]}"
    hrefs = [f"{base}{uids[0]}.ics", f"{base}{ghost_uid}.ics"]

    r = fresh_calendar.client.request(
        str(fresh_calendar.url),
        method="REPORT",
        body=_multiget_body(hrefs),
        headers={"Content-Type": "application/xml; charset=utf-8", "Depth": "1"},
    )
    assert 200 <= r.status < 300, (
        f"REPORT multiget with an unknown href must not 500 — got "
        f"HTTP {r.status}\nresponse: {r.raw!r}"
    )
    xml_body = r.raw.decode("utf-8") if isinstance(r.raw, bytes) else r.raw
    assert uids[0] in xml_body, (
        "Existing UID missing from multiget that also targeted a ghost href."
    )


# ─────────────────────────────────────────────────────────────
# Low-level helpers
# ─────────────────────────────────────────────────────────────


def _uid_from_event_data(data: str) -> str | None:
    """Pull the UID out of a raw iCalendar body. Cheap enough for a
    handful of events per test."""
    for line in data.replace("\r\n", "\n").split("\n"):
        if line.startswith("UID:"):
            return line[4:].strip()
    return None


def _multiget_body(hrefs: list[str]) -> str:
    """Assemble a minimal RFC 4791 §7.9 calendar-multiget REPORT
    XML body for the given href list."""
    href_xml = "\n    ".join(f"<D:href>{h}</D:href>" for h in hrefs)
    return f"""<?xml version="1.0" encoding="UTF-8"?>
<C:calendar-multiget xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop>
    <D:getetag/>
    <C:calendar-data/>
  </D:prop>
    {href_xml}
</C:calendar-multiget>
"""
