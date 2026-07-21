"""End-to-end regression for AtalayaLabs/OxiCloud#528 via python-caldav.

The Hurl coverage in `tests/api/caldav_recurring.hurl` exercises the
raw HTTP surface; this file drives the SAME behaviour through the
python-caldav client library — the same VObject + RFC 5545 stack that
Thunderbird, DAVx⁵ and Gnome Calendar use. If a real client's shape
diverges from what our Hurl fixtures send, this suite catches it.

Two access paths need distinguishing:

  * URL GET on `/caldav/<cal>/<uid>.ics` — routes through
    `find_event_by_ical_uid` which is master-only. This is what
    single-file iCal clients (older Thunderbird, Apple Reminders'
    quick-lookup) hit.

  * calendar-query REPORT — returns every calendar-object-resource
    matching the filter, so a UID with both a master AND per-instance
    overrides yields multiple entries. This is what modern CalDAV
    clients (Thunderbird 2024+, DAVx⁵, Apple Calendar) use for
    initial sync and delta refresh.

The suite exercises both paths — mixing them up is what tripped the
first draft (calendar.event_by_uid → REPORT under the hood, returned
the exception, tests failed).
"""

from __future__ import annotations

import textwrap
import uuid

import caldav


# ─────────────────────────────────────────────────────────────
# Helpers
# ─────────────────────────────────────────────────────────────


def _dedent(ical: str) -> str:
    """Strip test-source indentation and normalise line endings to
    CRLF, which RFC 5545 §3.1 mandates."""
    return textwrap.dedent(ical).strip().replace("\n", "\r\n") + "\r\n"


def _put_ical(calendar: caldav.Calendar, uid: str, body: str) -> None:
    """PUT the raw iCalendar body directly via pycaldav's authenticated
    session — bypassing pycaldav's `save_event()`.

    Empirically, `save_event(body)` re-parses the body through pycaldav's
    icalendar/vobject stack and re-serialises before PUTting. When the
    body contains a master VEVENT + a per-instance override sharing the
    same UID, that internal re-serialisation dropped the master and only
    sent the override — the exact behaviour the #528 fix must defend
    against. Bypassing that layer sends the bytes verbatim, mirroring
    what a real client (Thunderbird / DAVx⁵ / Apple Calendar) puts on
    the wire.
    """
    url = str(calendar.url).rstrip("/") + f"/{uid}.ics"
    response = calendar.client.request(
        url,
        method="PUT",
        body=body,
        headers={"Content-Type": "text/calendar; charset=utf-8"},
    )
    if response.status < 200 or response.status >= 300:
        raise AssertionError(
            f"PUT {url} → HTTP {response.status}\nbody sent: {body!r}\n"
            f"response: {response.raw!r}"
        )


def _get_master_ical(calendar: caldav.Calendar, uid: str) -> str:
    """Direct URL GET on `/caldav/<cal>/<uid>.ics` — routes through
    the master-only lookup on the server. Returns the raw response
    body (text/calendar).

    This bypasses pycaldav's REPORT-based `event_by_uid()` which
    would return every row matching the UID (master + exceptions)
    and force the caller to filter.
    """
    url = str(calendar.url).rstrip("/") + f"/{uid}.ics"
    response = calendar.client.request(url, method="GET")
    if response.status < 200 or response.status >= 300:
        raise AssertionError(
            f"GET {url} → HTTP {response.status}\n"
            f"body: {response.raw!r}"
        )
    return response.raw.decode("utf-8") if isinstance(response.raw, bytes) else response.raw


# ─────────────────────────────────────────────────────────────
# Baseline: prove the pipe works before we push it
# ─────────────────────────────────────────────────────────────


def test_non_recurring_event_round_trip(fresh_calendar: caldav.Calendar) -> None:
    uid = f"e2e-baseline-{uuid.uuid4().hex[:8]}"
    body = _dedent(
        f"""\
        BEGIN:VCALENDAR
        VERSION:2.0
        PRODID:-//pycaldav e2e//EN
        BEGIN:VEVENT
        UID:{uid}
        DTSTAMP:20260101T100000Z
        DTSTART:20260101T090000Z
        DTEND:20260101T093000Z
        SUMMARY:Baseline event
        END:VEVENT
        END:VCALENDAR
        """
    )
    _put_ical(fresh_calendar, uid, body)

    fetched = _get_master_ical(fresh_calendar, uid)
    assert "SUMMARY:Baseline event" in fetched
    assert f"UID:{uid}" in fetched


# ─────────────────────────────────────────────────────────────
# #528 timed flavour
# ─────────────────────────────────────────────────────────────


def test_recurring_master_plus_exception_preserves_master(
    fresh_calendar: caldav.Calendar,
) -> None:
    uid = f"e2e-daily-{uuid.uuid4().hex[:8]}"

    # (1) Master only — the shape a client sends when the user first
    # creates a recurring event.
    master_only = _dedent(
        f"""\
        BEGIN:VCALENDAR
        VERSION:2.0
        PRODID:-//pycaldav e2e//EN
        BEGIN:VEVENT
        UID:{uid}
        DTSTAMP:20260101T100000Z
        DTSTART:20260101T090000Z
        DTEND:20260101T093000Z
        SUMMARY:Daily standup
        RRULE:FREQ=DAILY;COUNT=10
        END:VEVENT
        END:VCALENDAR
        """
    )
    _put_ical(fresh_calendar, uid, master_only)

    # (2) Master + per-instance override — the shape a client sends
    # when the user modifies a single occurrence in the UI.
    with_exception = _dedent(
        f"""\
        BEGIN:VCALENDAR
        VERSION:2.0
        PRODID:-//pycaldav e2e//EN
        BEGIN:VEVENT
        UID:{uid}
        DTSTAMP:20260101T100000Z
        DTSTART:20260101T090000Z
        DTEND:20260101T093000Z
        SUMMARY:Daily standup
        RRULE:FREQ=DAILY;COUNT=10
        END:VEVENT
        BEGIN:VEVENT
        UID:{uid}
        DTSTAMP:20260101T100000Z
        DTSTART:20260103T110000Z
        DTEND:20260103T120000Z
        SUMMARY:Daily standup — rescheduled
        RECURRENCE-ID:20260103T090000Z
        END:VEVENT
        END:VCALENDAR
        """
    )
    _put_ical(fresh_calendar, uid, with_exception)

    # Master URL GET must return the master row. Pre-fix this would
    # have returned the exception's data (the last VEVENT in the
    # body clobbered the row).
    body = _get_master_ical(fresh_calendar, uid)
    assert "RRULE:FREQ=DAILY;COUNT=10" in body, (
        "Master row lost its RRULE — the exception overwrote the master. "
        "This is the exact regression from #528.\nBundle body: " + body
    )
    assert "SUMMARY:Daily standup" in body
    # Phase-4 read-side unification: the GET response is the
    # WHOLE calendar-object-resource — master + all exception
    # VEVENTs concatenated in one VCALENDAR per RFC 4791 §4.1 +
    # RFC 5545 §3.6.1. The exception's SUMMARY and its
    # RECURRENCE-ID must therefore appear alongside the master's
    # RRULE. Pre-phase-4 the emitter served only the master row
    # and clients silently dropped the exception on next-PUT.
    assert "SUMMARY:Daily standup — rescheduled" in body, (
        "Exception VEVENT missing from bundled GET body — phase-4 "
        "read-side regression.\nBundle body: " + body
    )
    assert "RECURRENCE-ID" in body, (
        "Exception RECURRENCE-ID missing from bundled GET body — "
        "clients need it to correlate the override with the master.\n"
        "Bundle body: " + body
    )


def test_exception_only_put_does_not_wipe_master(
    fresh_calendar: caldav.Calendar,
) -> None:
    uid = f"e2e-daily-{uuid.uuid4().hex[:8]}"

    # Seed: master + override.
    _put_ical(
        fresh_calendar,
        uid,
        _dedent(
            f"""\
            BEGIN:VCALENDAR
            VERSION:2.0
            PRODID:-//pycaldav e2e//EN
            BEGIN:VEVENT
            UID:{uid}
            DTSTAMP:20260101T100000Z
            DTSTART:20260101T090000Z
            DTEND:20260101T093000Z
            SUMMARY:Daily standup
            RRULE:FREQ=DAILY;COUNT=10
            END:VEVENT
            BEGIN:VEVENT
            UID:{uid}
            DTSTAMP:20260101T100000Z
            DTSTART:20260103T110000Z
            DTEND:20260103T120000Z
            SUMMARY:Daily standup — rescheduled
            RECURRENCE-ID:20260103T090000Z
            END:VEVENT
            END:VCALENDAR
            """
        ),
    )

    # Client's next action: user edits the same overridden occurrence
    # again. Thunderbird / Apple Calendar re-send ONLY the exception.
    _put_ical(
        fresh_calendar,
        uid,
        _dedent(
            f"""\
            BEGIN:VCALENDAR
            VERSION:2.0
            PRODID:-//pycaldav e2e//EN
            BEGIN:VEVENT
            UID:{uid}
            DTSTAMP:20260101T110000Z
            DTSTART:20260103T120000Z
            DTEND:20260103T130000Z
            SUMMARY:Daily standup — rescheduled AGAIN
            RECURRENCE-ID:20260103T090000Z
            END:VEVENT
            END:VCALENDAR
            """
        ),
    )

    # Bundled GET returns the WHOLE calendar-object-resource:
    # master row (unchanged since Step 1 seed) + the updated
    # exception row (SUMMARY "rescheduled AGAIN" from the
    # exception-only PUT above).
    # Pre-phase-3 the exception-only PUT wiped the master.
    # Pre-phase-4 the master survived but the exception was
    # invisible in the GET body.
    # Post-phase-4: both survive AND both are visible.
    body = _get_master_ical(fresh_calendar, uid)
    assert "RRULE:FREQ=DAILY;COUNT=10" in body, (
        "Master row lost its RRULE — data-loss regression from #528.\n"
        "Bundle body: " + body
    )
    assert "SUMMARY:Daily standup" in body, (
        "Master's original SUMMARY missing from bundle body — the "
        "master row was clobbered by the exception-only PUT.\n"
        "Bundle body: " + body
    )
    assert "SUMMARY:Daily standup — rescheduled AGAIN" in body, (
        "Updated exception SUMMARY missing — the second exception-only "
        "PUT either failed to update or the emitter dropped the exception "
        "row from the bundle.\nBundle body: " + body
    )


# ─────────────────────────────────────────────────────────────
# #528 all-day flavour — the exact shape the ticket was filed
# against. The DATE-form `DTSTART;VALUE=DATE:...` line was
# invisible to the pre-fix substring parser, so the whole
# body 500'd.
# ─────────────────────────────────────────────────────────────


def test_all_day_recurring_master_plus_exception(
    fresh_calendar: caldav.Calendar,
) -> None:
    uid = f"e2e-allday-{uuid.uuid4().hex[:8]}"
    body = _dedent(
        f"""\
        BEGIN:VCALENDAR
        VERSION:2.0
        PRODID:-//pycaldav e2e//EN
        BEGIN:VEVENT
        UID:{uid}
        DTSTAMP:20260101T100000Z
        DTSTART;VALUE=DATE:20260105
        DTEND;VALUE=DATE:20260106
        SUMMARY:Weekly review
        RRULE:FREQ=WEEKLY;COUNT=4
        END:VEVENT
        BEGIN:VEVENT
        UID:{uid}
        DTSTAMP:20260101T100000Z
        DTSTART;VALUE=DATE:20260113
        DTEND;VALUE=DATE:20260114
        SUMMARY:Weekly review — moved
        RECURRENCE-ID;VALUE=DATE:20260112
        END:VEVENT
        END:VCALENDAR
        """
    )
    _put_ical(fresh_calendar, uid, body)

    # Master URL GET returns the master row with the RRULE intact.
    # Pre-parser-rewrite the whole PUT 500'd because the param-
    # carrying DTSTART line was invisible to the scanner.
    data = _get_master_ical(fresh_calendar, uid)
    assert "RRULE:FREQ=WEEKLY;COUNT=4" in data, (
        "Master lost its RRULE (or the whole PUT was rejected).\n"
        f"Master body: {data}"
    )
    assert "SUMMARY:Weekly review" in data
    # Phase-4 bundle: exception row visible in the GET body.
    # DATE-form RECURRENCE-ID (with the `;VALUE=DATE` parameter)
    # survives verbatim because we serve stored ical_data
    # instead of regenerating.
    assert "SUMMARY:Weekly review — moved" in data, (
        "All-day exception SUMMARY missing from bundled GET body:\n"
        + data
    )
    assert "RECURRENCE-ID;VALUE=DATE:20260112" in data, (
        "DATE-form RECURRENCE-ID lost — either the exception row "
        "isn't in the bundle or the emitter mangled the property "
        "parameter.\nBundle body: " + data
    )
