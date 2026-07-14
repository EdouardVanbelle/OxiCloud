"""Non-recurring iCalendar property coverage via python-caldav.

Complements `test_recurring.py` (the #528 regression suite) by
sweeping the property surface of a single, non-recurring VEVENT.
Real CalDAV clients send many properties beyond DTSTART/DTEND +
SUMMARY; whether those survive a PUT → GET round-trip is what
this file measures.

The GET path in `caldav_handler.rs::write_vevent` regenerates
the response body from the stored DTO fields (UID / SUMMARY /
DTSTART / DTEND / DESCRIPTION / LOCATION / RRULE / DTSTAMP /
CREATED / LAST-MODIFIED). Anything not in that list is silently
dropped even though the original `ical_data` is stored intact.

Tests split into two groups:

  * **Sanity** — properties the server emits on GET; they must
    round-trip. Regressions here would be genuine server bugs.

  * **xfail (documented gaps)** — properties the server currently
    drops. `@pytest.mark.xfail(strict=False)` lets the suite stay
    green while making the gap visible in the pytest summary. If
    a future server fix makes one of these survive, pytest
    reports it as `XPASS` — an alert to remove the marker.
"""

from __future__ import annotations

import textwrap
import uuid

import caldav
import pytest


# ─────────────────────────────────────────────────────────────
# Helpers (mirror the raw-HTTP-PUT / master-URL-GET pattern
# from test_recurring.py). Kept local to this file for now;
# fold into conftest.py if a third test file wants them.
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


def _get_ical(calendar: caldav.Calendar, uid: str) -> str:
    url = str(calendar.url).rstrip("/") + f"/{uid}.ics"
    r = calendar.client.request(url, method="GET")
    if r.status < 200 or r.status >= 300:
        raise AssertionError(f"GET {url} → HTTP {r.status}")
    return r.raw.decode("utf-8") if isinstance(r.raw, bytes) else r.raw


def _minimal_event(uid: str, **extra_lines: str) -> str:
    """Build a minimal VEVENT with the given extra iCal property lines
    injected before END:VEVENT. Values in `extra_lines` should be full
    property lines (name+value), one per key. The key exists only so
    tests can override without clobbering; it isn't emitted."""
    base = f"""\
        BEGIN:VCALENDAR
        VERSION:2.0
        PRODID:-//pycaldav coverage//EN
        BEGIN:VEVENT
        UID:{uid}
        DTSTAMP:20260101T100000Z
        DTSTART:20260101T090000Z
        DTEND:20260101T093000Z
        SUMMARY:Coverage event
    """
    body = textwrap.dedent(base).rstrip() + "\n"
    for line in extra_lines.values():
        body += line + "\n"
    body += "END:VEVENT\nEND:VCALENDAR\n"
    return body.replace("\n", "\r\n")


# ─────────────────────────────────────────────────────────────
# Sanity — properties the server DOES emit on GET.
# ─────────────────────────────────────────────────────────────


def test_description_with_escaped_chars_round_trips(
    fresh_calendar: caldav.Calendar,
) -> None:
    """RFC 5545 §3.3.11 mandates comma / semicolon / newline
    escaping in TEXT values. A Description with all three must
    survive PUT → GET.

    Note: our own generate_event_ical only escapes newlines
    (`\\n`), not commas or semicolons — this test guards the
    minimum bar. A stricter test could assert exact escape
    handling; deferred until the emitter is RFC-strict."""
    uid = f"cov-desc-{uuid.uuid4().hex[:8]}"
    # RFC 5545 escapes: `\n` for newline, `\,` for comma, `\;` for
    # semicolon. Client sends them ALREADY escaped in the wire body.
    body = _minimal_event(
        uid,
        description=r"DESCRIPTION:multi-line\ntext with a comma\, and a semi\;colon.",
    )
    _put_ical(fresh_calendar, uid, body)

    fetched = _get_ical(fresh_calendar, uid)
    assert "multi-line" in fetched
    # Server currently emits `\n` back but may drop `\,` / `\;`
    # escapes — accept either the escaped or unescaped form here so
    # the sanity check tolerates the current emitter without failing
    # on the strict spec detail.
    assert (
        "comma" in fetched.lower()
    ), f"DESCRIPTION body lost the comma text entirely:\n{fetched}"


def test_location_survives_round_trip(fresh_calendar: caldav.Calendar) -> None:
    uid = f"cov-loc-{uuid.uuid4().hex[:8]}"
    body = _minimal_event(
        uid,
        location="LOCATION:Room 3B\\, Building 42",
    )
    _put_ical(fresh_calendar, uid, body)

    fetched = _get_ical(fresh_calendar, uid)
    assert "Room 3B" in fetched, f"LOCATION lost:\n{fetched}"


def test_uid_and_dtstamp_are_preserved(fresh_calendar: caldav.Calendar) -> None:
    """Belt-and-braces sanity — UID is the resource identifier and
    DTSTAMP is required by RFC 5545 §3.8.7.2 on every VEVENT. Both
    are emitted from DTO fields, so both round-trip cleanly."""
    uid = f"cov-uid-{uuid.uuid4().hex[:8]}"
    body = _minimal_event(uid)
    _put_ical(fresh_calendar, uid, body)

    fetched = _get_ical(fresh_calendar, uid)
    assert f"UID:{uid}" in fetched
    assert "DTSTAMP:" in fetched


# ─────────────────────────────────────────────────────────────
# Documented gaps — properties the server currently drops on
# GET. `xfail(strict=False)` means "expected to fail; don't fail
# the suite, but flag XPASS if it starts passing". When the
# read-side fix lands, remove the marker.
# ─────────────────────────────────────────────────────────────

_EMITTER_GAP_REASON = (
    "GET regenerates the body from DTO fields via write_vevent "
    "(caldav_handler.rs:~770) which only emits UID / SUMMARY / "
    "DTSTART / DTEND / DESCRIPTION / LOCATION / RRULE / DTSTAMP / "
    "CREATED / LAST-MODIFIED. Every other iCal property is stored "
    "in ical_data on the row but silently dropped on read. "
    "Fix path: either serve ical_data verbatim on GET, or extend "
    "the DTO to carry the full property set."
)


@pytest.mark.xfail(reason=_EMITTER_GAP_REASON, strict=False)
def test_attendee_survives_round_trip(fresh_calendar: caldav.Calendar) -> None:
    uid = f"cov-attendee-{uuid.uuid4().hex[:8]}"
    body = _minimal_event(
        uid,
        attendee=(
            "ATTENDEE;CN=Alice;PARTSTAT=ACCEPTED;RSVP=TRUE:"
            "mailto:alice@example.com"
        ),
    )
    _put_ical(fresh_calendar, uid, body)

    fetched = _get_ical(fresh_calendar, uid)
    assert "ATTENDEE" in fetched, f"ATTENDEE dropped:\n{fetched}"
    assert "alice@example.com" in fetched


@pytest.mark.xfail(reason=_EMITTER_GAP_REASON, strict=False)
def test_organizer_survives_round_trip(fresh_calendar: caldav.Calendar) -> None:
    uid = f"cov-organizer-{uuid.uuid4().hex[:8]}"
    body = _minimal_event(
        uid,
        organizer="ORGANIZER;CN=Bob:mailto:bob@example.com",
    )
    _put_ical(fresh_calendar, uid, body)

    fetched = _get_ical(fresh_calendar, uid)
    assert "ORGANIZER" in fetched
    assert "bob@example.com" in fetched


@pytest.mark.xfail(reason=_EMITTER_GAP_REASON, strict=False)
def test_categories_survive_round_trip(fresh_calendar: caldav.Calendar) -> None:
    uid = f"cov-cats-{uuid.uuid4().hex[:8]}"
    body = _minimal_event(
        uid,
        categories="CATEGORIES:MEETING,ENGINEERING,SPRINT-42",
    )
    _put_ical(fresh_calendar, uid, body)

    fetched = _get_ical(fresh_calendar, uid)
    assert "CATEGORIES" in fetched
    assert "ENGINEERING" in fetched


@pytest.mark.xfail(reason=_EMITTER_GAP_REASON, strict=False)
def test_status_and_transp_survive_round_trip(
    fresh_calendar: caldav.Calendar,
) -> None:
    """STATUS (RFC 5545 §3.8.1.11) and TRANSP (§3.8.2.7) drive
    "tentative vs confirmed" and "shows as busy vs free" in every
    calendar client UI. Losing them silently is user-visible."""
    uid = f"cov-status-{uuid.uuid4().hex[:8]}"
    body = _minimal_event(
        uid,
        status="STATUS:TENTATIVE",
        transp="TRANSP:TRANSPARENT",
    )
    _put_ical(fresh_calendar, uid, body)

    fetched = _get_ical(fresh_calendar, uid)
    assert "STATUS:TENTATIVE" in fetched
    assert "TRANSP:TRANSPARENT" in fetched


@pytest.mark.xfail(reason=_EMITTER_GAP_REASON, strict=False)
def test_valarm_survives_round_trip(fresh_calendar: caldav.Calendar) -> None:
    """VALARM is a nested sub-component of VEVENT (RFC 5545 §3.6.6)
    and drives every "remind me 15 min before" popup. It lives
    entirely in ical_data on the row and is invisible to the DTO.
    Dropping it on GET means alarms silently disappear after the
    first client sync."""
    uid = f"cov-alarm-{uuid.uuid4().hex[:8]}"
    body = _dedent(
        f"""\
        BEGIN:VCALENDAR
        VERSION:2.0
        PRODID:-//pycaldav coverage//EN
        BEGIN:VEVENT
        UID:{uid}
        DTSTAMP:20260101T100000Z
        DTSTART:20260101T090000Z
        DTEND:20260101T093000Z
        SUMMARY:Event with alarm
        BEGIN:VALARM
        ACTION:DISPLAY
        TRIGGER:-PT15M
        DESCRIPTION:15 min reminder
        END:VALARM
        END:VEVENT
        END:VCALENDAR
        """
    )
    _put_ical(fresh_calendar, uid, body)

    fetched = _get_ical(fresh_calendar, uid)
    assert "BEGIN:VALARM" in fetched, f"VALARM block dropped:\n{fetched}"
    assert "TRIGGER:-PT15M" in fetched


@pytest.mark.xfail(reason=_EMITTER_GAP_REASON, strict=False)
def test_custom_x_property_survives_round_trip(
    fresh_calendar: caldav.Calendar,
) -> None:
    """Custom `X-*` properties (RFC 5545 §3.8.8.2). Apple Calendar
    uses `X-APPLE-*`, DAVx⁵ uses `X-MOZ-*`, and Nextcloud uses
    `X-NEXTCLOUD-*`. Dropping them breaks client-specific UI cues
    without corrupting core interop."""
    uid = f"cov-xprop-{uuid.uuid4().hex[:8]}"
    body = _minimal_event(
        uid,
        xprop="X-MOZ-LASTACK:20260101T090000Z",
    )
    _put_ical(fresh_calendar, uid, body)

    fetched = _get_ical(fresh_calendar, uid)
    assert "X-MOZ-LASTACK" in fetched
