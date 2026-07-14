"""CardDAV (RFC 6352) surface coverage.

python-caldav has no CardDAV support (the library name is a bit
misleading — it's CalDAV-only). These tests drive the server via
raw HTTP through the SAME authenticated `dav_client` session used
by the CalDAV tests, so credentials + connection reuse stay
consistent with the rest of the suite.

Fixtures:
  * `carddav_url` — CardDAV base URL, derived from OXICLOUD_CALDAV_URL
    by replacing `/caldav/` with `/carddav/`.
  * `fresh_addressbook` — a brand-new address book per test; yields
    the server-authoritative URL as a string; teardown DELETEs it.

Coverage: the sanity tests cover the FN/N/EMAIL core; the
extended round-trips (ORG/TITLE/NOTE/TEL/ADR) each pin a
parser + emitter pair. Any regression that drops a property
on the round-trip fails the corresponding test.

The CardDAV emitter regenerates vCard bodies from stored DTO
fields on GET — properties without a DTO field (BDAY, PHOTO,
categories, custom X-*) still don't survive. Same emitter-gap
class as `test_ical_coverage.py`; would be closed by serving
the stored vcard_data verbatim or extending the DTO.
"""

from __future__ import annotations

import textwrap
import uuid

import caldav


# ─────────────────────────────────────────────────────────────
# Helpers — mirror the CalDAV pattern. Raw HTTP through the
# authenticated pycaldav session; no client-library abstractions.
# ─────────────────────────────────────────────────────────────


def _dedent_vcard(body: str) -> str:
    """RFC 6350 §3.2 mandates CRLF between properties, same as
    iCalendar. Normalise text-block indentation and line endings."""
    return textwrap.dedent(body).strip().replace("\n", "\r\n") + "\r\n"


def _put_vcard(
    dav_client: caldav.DAVClient, addressbook_url: str, uid: str, body: str
) -> None:
    url = addressbook_url.rstrip("/") + f"/{uid}.vcf"
    r = dav_client.request(
        url,
        method="PUT",
        body=body,
        headers={"Content-Type": "text/vcard; charset=utf-8"},
    )
    if r.status < 200 or r.status >= 300:
        raise AssertionError(
            f"PUT {url} → HTTP {r.status}\nbody: {body!r}\nresponse: {r.raw!r}"
        )


def _get_vcard(
    dav_client: caldav.DAVClient, addressbook_url: str, uid: str
) -> str:
    url = addressbook_url.rstrip("/") + f"/{uid}.vcf"
    r = dav_client.request(url, method="GET")
    if r.status < 200 or r.status >= 300:
        raise AssertionError(f"GET {url} → HTTP {r.status}\n{r.raw!r}")
    return r.raw.decode("utf-8") if isinstance(r.raw, bytes) else r.raw


def _delete_vcard(
    dav_client: caldav.DAVClient, addressbook_url: str, uid: str
) -> int:
    url = addressbook_url.rstrip("/") + f"/{uid}.vcf"
    r = dav_client.request(url, method="DELETE")
    return r.status


def _minimal_vcard(uid: str, **extras: str) -> str:
    """Build a minimal RFC 6350 vCard 4.0 body with the given
    extra property lines injected before END:VCARD."""
    base = f"""\
        BEGIN:VCARD
        VERSION:4.0
        UID:{uid}
        FN:Coverage Contact
        N:Coverage;Contact;;;
    """
    body = textwrap.dedent(base).rstrip() + "\n"
    for line in extras.values():
        body += line + "\n"
    body += "END:VCARD\n"
    return body.replace("\n", "\r\n")


# ─────────────────────────────────────────────────────────────
# Sanity — properties the server round-trips.
# ─────────────────────────────────────────────────────────────


def test_vcard_basic_round_trip(
    dav_client: caldav.DAVClient, fresh_addressbook: str
) -> None:
    """The core CardDAV contract: PUT a vCard, GET it back, body
    contains at least the UID + FN we sent. FN (formatted name)
    is RFC 6350 §6.2.1 REQUIRED — a vCard without it is invalid,
    and the server must preserve it verbatim."""
    uid = f"cov-basic-{uuid.uuid4().hex[:8]}"
    body = _minimal_vcard(uid)
    _put_vcard(dav_client, fresh_addressbook, uid, body)

    fetched = _get_vcard(dav_client, fresh_addressbook, uid)
    assert f"UID:{uid}" in fetched, f"UID missing from GET:\n{fetched}"
    assert "FN:Coverage Contact" in fetched, (
        f"FN dropped on round-trip:\n{fetched}"
    )


def test_vcard_email_survives_round_trip(
    dav_client: caldav.DAVClient, fresh_addressbook: str
) -> None:
    """EMAIL (RFC 6350 §6.4.2) — one of the two properties most
    real contact clients set. Loss here would break sync with
    every address-book UI."""
    uid = f"cov-email-{uuid.uuid4().hex[:8]}"
    body = _minimal_vcard(
        uid,
        email="EMAIL;TYPE=work:coverage.contact@example.com",
    )
    _put_vcard(dav_client, fresh_addressbook, uid, body)

    fetched = _get_vcard(dav_client, fresh_addressbook, uid)
    assert "coverage.contact@example.com" in fetched, (
        f"EMAIL dropped on round-trip:\n{fetched}"
    )


def test_vcard_delete_removes_it(
    dav_client: caldav.DAVClient, fresh_addressbook: str
) -> None:
    """PUT → DELETE → GET must 404. Regression guard against
    delete-doesn't-actually-delete bugs (which have surfaced in
    other DAV surfaces during D7 work)."""
    uid = f"cov-del-{uuid.uuid4().hex[:8]}"
    _put_vcard(dav_client, fresh_addressbook, uid, _minimal_vcard(uid))

    status = _delete_vcard(dav_client, fresh_addressbook, uid)
    assert 200 <= status < 300, f"DELETE returned HTTP {status}"

    # Re-fetch should 404. `_get_vcard` raises on non-2xx; catch it.
    url = fresh_addressbook.rstrip("/") + f"/{uid}.vcf"
    r = dav_client.request(url, method="GET")
    assert r.status == 404, (
        f"GET after DELETE expected 404; got HTTP {r.status}"
    )


def test_addressbook_shows_up_in_propfind(
    dav_client: caldav.DAVClient,
    carddav_url: str,
    fresh_addressbook: str,
) -> None:
    """Sanity: the just-created address book is listed by a
    PROPFIND Depth 1 on the CardDAV root. Same shape a real
    client uses to enumerate address books at login."""
    propfind_body = (
        '<?xml version="1.0" encoding="UTF-8"?>'
        '<D:propfind xmlns:D="DAV:">'
        "<D:prop><D:displayname/><D:resourcetype/></D:prop>"
        "</D:propfind>"
    )
    r = dav_client.request(
        carddav_url,
        method="PROPFIND",
        body=propfind_body,
        headers={"Depth": "1", "Content-Type": "application/xml"},
    )
    assert 200 <= r.status < 300, f"PROPFIND → HTTP {r.status}"
    xml = r.raw.decode("utf-8") if isinstance(r.raw, bytes) else r.raw

    # `fresh_addressbook` is an absolute URL; the href in the
    # PROPFIND response is the path portion. Extract and check.
    import urllib.parse

    ab_path = urllib.parse.urlparse(fresh_addressbook).path
    assert ab_path in xml, (
        f"Fresh address book path {ab_path} missing from PROPFIND:\n{xml}"
    )


# ─────────────────────────────────────────────────────────────
# Extended round-trips — properties beyond the FN/N/EMAIL core.
# Each has a parse_vcard branch + a contact_to_vcard emitter
# branch; loss of any of these on a real-client sync would
# silently break the corresponding UI slot (job title, phone,
# address, notes).
# ─────────────────────────────────────────────────────────────


def test_vcard_org_and_title_survive_round_trip(
    dav_client: caldav.DAVClient, fresh_addressbook: str
) -> None:
    """ORG + TITLE (RFC 6350 §6.6.4 / §6.6.1). Business-card
    fields — losing them means everyone's job title disappears
    from address-book UIs after the first sync.

    Passes today: parse_vcard has ORG / TITLE branches; the
    emitter (contact_to_vcard) rewrites both from DTO fields."""
    uid = f"cov-org-{uuid.uuid4().hex[:8]}"
    body = _minimal_vcard(
        uid,
        org="ORG:Acme Corporation;R&D",
        title="TITLE:Principal Engineer",
    )
    _put_vcard(dav_client, fresh_addressbook, uid, body)

    fetched = _get_vcard(dav_client, fresh_addressbook, uid)
    assert "Acme Corporation" in fetched
    assert "Principal Engineer" in fetched


def test_vcard_note_survives_round_trip(
    dav_client: caldav.DAVClient, fresh_addressbook: str
) -> None:
    """NOTE (RFC 6350 §6.7.2). Free-form text field every contact
    UI exposes. Passes today: parse_vcard strips NOTE:, emitter
    re-emits with newline escaping."""
    uid = f"cov-note-{uuid.uuid4().hex[:8]}"
    body = _minimal_vcard(
        uid,
        note="NOTE:Met at KubeCon 2026. Prefers email over phone.",
    )
    _put_vcard(dav_client, fresh_addressbook, uid, body)

    fetched = _get_vcard(dav_client, fresh_addressbook, uid)
    assert "KubeCon 2026" in fetched


def test_vcard_tel_uri_form_survives_round_trip(
    dav_client: caldav.DAVClient, fresh_addressbook: str
) -> None:
    """TEL (RFC 6350 §6.4.1) with URI-form value + TYPE parameter —
    the shape Apple Contacts / DAVx⁵ send for every phone number.

    Passes after fix/carddav-parser-tel-adr: parse_vcard splits on
    the first `:` (was `split(':').nth(1)`) so `VALUE=uri:tel:...`
    survives; the `tel:` URI scheme is stripped so the stored
    number is a bare `+15551234567`."""
    uid = f"cov-tel-{uuid.uuid4().hex[:8]}"
    body = _minimal_vcard(
        uid,
        tel="TEL;TYPE=cell;VALUE=uri:tel:+15551234567",
    )
    _put_vcard(dav_client, fresh_addressbook, uid, body)

    fetched = _get_vcard(dav_client, fresh_addressbook, uid)
    assert "+15551234567" in fetched


def test_vcard_adr_survives_round_trip(
    dav_client: caldav.DAVClient, fresh_addressbook: str
) -> None:
    """ADR (RFC 6350 §6.3.1) with structured components. Semicolon
    is the structured-value separator.

    Passes after fix/carddav-parser-tel-adr: the parser now has an
    ADR branch that splits the 7-part structured value into
    (street, city, state, postal_code, country) matching the
    emitter shape at contact_service.rs::generate_vcard."""
    uid = f"cov-adr-{uuid.uuid4().hex[:8]}"
    body = _minimal_vcard(
        uid,
        adr="ADR;TYPE=home:;;42 Rue de Rivoli;Paris;;75001;France",
    )
    _put_vcard(dav_client, fresh_addressbook, uid, body)

    fetched = _get_vcard(dav_client, fresh_addressbook, uid)
    assert "Rue de Rivoli" in fetched
    assert "Paris" in fetched
