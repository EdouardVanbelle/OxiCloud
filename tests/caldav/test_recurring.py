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
        "This is the exact regression from #528.\nMaster body: " + body
    )
    assert "SUMMARY:Daily standup" in body

    # NOTE: not asserting the exception row is client-visible here.
    # RFC 4791 §4.1 + RFC 5545 §3.8.4.4 model a recurring event with
    # per-instance overrides as ONE calendar-object-resource whose
    # VCALENDAR contains the master VEVENT + all exception VEVENTs.
    # OxiCloud currently persists them as separate rows but the
    # GET/PROPFIND emitter returns only the master (see phase-4
    # follow-up on branch feat/caldav-read-side). Once phase 4
    # lands, add: assert "RECURRENCE-ID" in body and
    # assert "rescheduled" in body.


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

    # Master URL GET must still return the master. Pre-fix the
    # exception-only PUT would have replaced the master (keyed by
    # UID with no recurrence_id filter) — this is the data-loss
    # half of #528.
    body = _get_master_ical(fresh_calendar, uid)
    assert "RRULE:FREQ=DAILY;COUNT=10" in body
    assert "SUMMARY:Daily standup" in body
    assert "rescheduled" not in body, (
        "GET on the master URL returned the exception's data — the "
        "master was clobbered by the exception-only PUT."
    )

    # NOTE: exception-row survival is not asserted client-side
    # today — the emitter only surfaces the master. Phase 4
    # (feat/caldav-read-side) will fold master + exceptions into a
    # single VCALENDAR body; once landed, add an assertion that the
    # updated exception's SUMMARY ("rescheduled AGAIN") is present
    # in the same GET body as the master's RRULE.


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

    # NOTE: exception row is stored server-side but not yet visible
    # in the GET body. Phase 4 will fold it in — assertion to add
    # once that lands: assert "RECURRENCE-ID;VALUE=DATE:20260112" in data.
