# =============================================================
# OxiCloud — CardDAV vCard property round-trip regression
# =============================================================
# Regression pin for fix/carddav-parser-tel-adr.
#
# `contact_service.rs::parse_vcard` had two independent gaps
# and one case-sensitivity issue on the TYPE parameter:
#
#   1. TEL used `split(':').nth(1)` — a URI-form value like
#      `TEL;TYPE=cell;VALUE=uri:tel:+15551234567` was sliced
#      down to `"tel"`, losing the phone number entirely.
#   2. ADR had no parser branch at all — every address was
#      silently dropped at PUT time.
#   3. TYPE param matching was case-sensitive; real clients
#      (Apple Contacts, DAVx⁵, python-caldav) mix cases so
#      `TYPE=cell` fell through to "other" instead of "mobile".
#
# Post-fix: `splitn(2, ':')` + `tel:` scheme strip, an ADR
# branch parsing the 7-part structured value into (street,
# city, state, postal_code, country), and case-insensitive
# TYPE matching (uppercased once, checked against upper).
#
# Test shape: PUT a vCard exercising all three fixes; GET it
# back; assert the emitter surfaces the parsed fields.
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
# Step 2 — Discover admin's default address book.
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/address-books
Authorization: Bearer {{admin_token}}

HTTP 200
[Captures]
# See dav_error_mapping.hurl for why body regex vs jsonpath
# filter — same rationale (Hurl's scalar-vs-list handling on
# single-match jsonpath filters is brittle).
default_book_id: body regex "\"id\":\"([a-f0-9-]{36})\",\"name\":\"Contacts\""


# ─────────────────────────────────────────────────────────────
# Step 3 — PUT a vCard exercising all three fixes:
#   * TEL URI form with lowercase TYPE=cell (URI-scheme + case
#     insensitivity).
#   * ADR with a full 7-field structured value and TYPE=HOME
#     (parser branch existence + type detection).
#   * EMAIL as a sanity anchor — the pre-existing path we did
#     NOT change; must still round-trip cleanly.
#
# `dav-err-` UID prefix so a re-run inside the same DB (this
# file runs BEFORE contacts.hurl in run.sh, so its state
# doesn't collide with that suite).
# ─────────────────────────────────────────────────────────────
PUT {{base_url}}/carddav/{{default_book_id}}/dav-err-vcard-props.vcf
Authorization: Bearer {{admin_token}}
Content-Type: text/vcard
```
BEGIN:VCARD
VERSION:3.0
UID:dav-err-vcard-props
FN:Regression VCard
N:VCard;Regression;;;
EMAIL;TYPE=work:regression@example.com
TEL;TYPE=cell;VALUE=uri:tel:+15551234567
ADR;TYPE=HOME:;;42 Rue de Rivoli;Paris;Île-de-France;75001;France
END:VCARD
```

HTTP *
[Asserts]
status >= 200
status < 300


# ─────────────────────────────────────────────────────────────
# Step 4 — GET the vCard back and assert the parser+emitter
# preserved each property. The emitter regenerates the body
# from DTO fields, so a value surfacing in the response body
# is proof it made it through parser → DB → emitter intact.
#
# NOTE: emitter uses vCard 3.0 uppercase TYPE values
# (`TEL;TYPE=MOBILE:`, `ADR;TYPE=HOME:`), so the response
# body's casing is normalised regardless of what the client
# sent.
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/carddav/{{default_book_id}}/dav-err-vcard-props.vcf
Authorization: Bearer {{admin_token}}

HTTP 200
[Asserts]
# The bare number, with URI scheme stripped and unchanged
# through the parser's first-colon split. Pre-fix this would
# have been `+15551234567` in the input but the DB would
# store `"tel"` and the emitter would output that instead.
body contains "+15551234567"
# Case-insensitive TYPE detection: lowercase `TYPE=cell` on
# input → mapped to "mobile" internally → emitter writes
# uppercase `TYPE=MOBILE`. Pre-fix (case-sensitive) this fell
# through to "other" and emitted `TYPE=OTHER`.
body contains "TYPE=MOBILE"
# Address components — proves the ADR parser branch runs.
body contains "42 Rue de Rivoli"
body contains "Paris"
body contains "Île-de-France"
body contains "75001"
body contains "France"
# TYPE=HOME preserved from the input (uppercase in both
# directions).
body contains "TYPE=HOME"
# Sanity: pre-existing EMAIL path still round-trips.
body contains "regression@example.com"


# ─────────────────────────────────────────────────────────────
# Step 5 — Cleanup: delete the vCard so downstream files
# (contacts.hurl in particular) don't inherit the fixture.
# ─────────────────────────────────────────────────────────────
DELETE {{base_url}}/carddav/{{default_book_id}}/dav-err-vcard-props.vcf
Authorization: Bearer {{admin_token}}

HTTP 204
