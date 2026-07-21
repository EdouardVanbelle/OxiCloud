# Round 19 — auth/WOPI/vCard/PROPFIND per-request & per-row alloc cuts

Benchmark-gated, same rule as ROUND2–18: every change ships with a BEFORE/AFTER
benchmark and an equivalence/safety gate; an AFTER that doesn't beat its BEFORE
is rolled back (never applied). The roll-back rule is encoded directly into the
harness — a `GATE FAIL … rollback` non-zero exit if an AFTER arm fails to reduce
allocations (or, for the CPU-only §V2 stamp, fails to beat BEFORE by the required
wall ratio) — so a regression fails CI rather than shipping.

This round sweeps the **per-request** DAV/WOPI/NextCloud plumbing and two
**per-row** emit loops the earlier rounds' handler passes left untouched. Every
item mirrors an optimization the codebase already proved out elsewhere
(`JwtTokenService`'s prebuilt keys, `common::fmt`'s stack date renderers, the
favorites/recent/folder row mappers' move-not-clone, the CalDAV emitter's reused
per-row buffers) but which never reached these specific paths.

Reproduce:

```
cargo run --release --features bench --example bench_round19_micro
```

All arms are **no-Postgres** (release-profile counting-allocator example).

## Summary

| # | change | key metric | before → after |
|--:|---|---|---|
| **M1** | `AppPasswordService::verify_basic_auth` built the moka cache key as `blake3::hash(format!("{username}:{password}").as_bytes())` — one throwaway `String` per **Basic-auth request** (runs before the cache lookup, so even hits pay it; DAV sync clients hammer it on every request). Now streamed into an incremental `blake3::Hasher` — byte-identical 32-byte key. | 20-byte creds | **2 → 0 allocs/op · 1.66× wall** (182.0 → 109.4 ns) |
| **M2** | `WopiTokenService::validate_token`/`generate_token` rebuilt a `Validation` (allocates a `required_spec_claims` HashSet + `algorithms` Vec) and a `DecodingKey`/`EncodingKey` (copies the secret into a fresh Vec) on **every WOPI call** — Office/Collabora poll continuously. Now all three are prebuilt struct fields in `new()` (exactly what `JwtTokenService` already does). | HS256 validate | **16 → 12 allocs/op · 1.07× wall** |
| **V1** | `contact_to_vcard`/`generate_vcard`, **per contact** in every CardDAV REPORT/multiget/PROPFIND-with-address-data: FN fallback dropped the throwaway `.to_string()` copy of the trim slice; NOTE `replace('\n', "\\n")` is now guarded (`contains('\n')`) so a newline-free note writes borrowed; REV `.format("%Y%m%dT%H%M%SZ")` → `common::fmt::compact_ical_utc`. | full vCard emit | **9 → 4 allocs/op · 1.97× wall** (548.2 → 277.7 ns) |
| **V2** | The REV/DTSTAMP stamp isolated: chrono `.format("%Y%m%dT%H%M%SZ")` runs the strftime interpreter and (measured) **allocates 3×** per call; the new `common::fmt::compact_ical_utc` renders `YYYYMMDDTHHMMSSZ` into a 16-byte stack buffer via the shared `push2`/`push4` LUT. | one stamp | **3 → 0 allocs/op · 11.77× wall** (216.9 → 18.4 ns) |
| **M4** | `trash_service::row_to_item_dto` `clone()`d `name`/`path`/`blob_hash` out of an **owned** `row` that is dropped at fn end — 2 clones/folder row, 3/file row, up to 200 rows/`/api/trash` page. Now moved (the favorites/recent/folder mappers already move these). | file row | **10 → 7 allocs/op** (3 clones gone) |
| **M5** | `SearchUseCase::search` built the cache-key user segment via `user_id.to_string()` — one heap `String` **per search request** to feed a hasher the fn doc even calls "zero-allocation". Now stack-encoded via `Uuid::hyphenated().encode_lower(&mut [u8; 36])`; byte-identical string ⇒ identical u64 key. | 1 request | **1 → 0 allocs/op · 1.30× wall** |
| **M6** | Streaming WebDAV **PROPFIND** built each child `href` with a fresh `format!` per row — up to 500 rows/page, 4 loops across the native + NextCloud handlers, the single most-travelled DAV path. Now one buffer reused across the page (`clear` + `push_str` + `extend`/`push_str`). | 64-child page | **192 → 3 allocs/op · 2.74× wall** (10.9 → 4.0 µs) |
| **M7** | `nextcloud::session::extract_url_user` forced `.into_owned()` on the `urlencoding::decode` `Cow` on **every path-scoped NC DAV request**, though a plain-ASCII username decodes to `Cow::Borrowed`. Now returns the `Cow` and compares by `.as_ref()`. | ASCII user | **1 → 0 allocs/op · 3.11× wall** (25.5 → 8.2 ns) |

> Allocs/op is the deterministic primary gate (identical run to run). Wall
> figures are single-shot and noise-bounded; §V2 is the one CPU-only arm (both
> emit the same 0 allocs after the fix is measured against chrono's 3) and is
> gated on a ≥2× wall ratio — it clears it with 11.8×.

## [M1] Basic-auth cache key — incremental hasher

`verify_basic_auth` runs on every WebDAV/CalDAV/CardDAV/NextCloud request that
carries Basic auth — and DAV sync clients (DAVx5, Apple, Thunderbird, the
Nextcloud desktop client) send credentials on **every** request, holding 4–8
parallel connections. The cache key is computed *before* the single-flight cache
lookup, so it runs on hits too:

```rust
let cache_key: [u8; 32] =
    blake3::hash(format!("{}:{}", username, password).as_bytes()).into();
```

The `format!` heap-allocates one `String` per request purely to concatenate the
two parts before handing the bytes to blake3. blake3 is a **streaming** hash —
feeding `username`, then `":"`, then `password` into an incremental `Hasher`
produces the identical digest with no intermediate buffer:

```rust
let cache_key: [u8; 32] = {
    let mut h = blake3::Hasher::new();
    h.update(username.as_bytes());
    h.update(b":");
    h.update(password.as_bytes());
    h.finalize().into()
};
```

The bench's equivalence gate asserts the two 32-byte keys are identical, so
in-flight and cached entries collide exactly as before. **2 → 0 allocs/op,
1.66× wall** — and note the `format!` version's *second* alloc is the
`String`'s grow, both gone.

## [M2] WOPI token validate/generate — prebuilt keys

`WopiTokenService` mirrored none of the prebuilt-key discipline
`JwtTokenService` adopted in an earlier round. Every `validate_token` (6 WOPI
handler entry points — CheckFileInfo, GetFile, PutFile, Lock, …, polled
continuously by the Office/Collabora host during an edit session) rebuilt:

```rust
let validation = Validation::new(Algorithm::HS256);      // HashSet + Vec
let token_data = decode::<WopiTokenClaims>(
    token,
    &DecodingKey::from_secret(self.secret.as_bytes()),   // fresh Vec copy of the secret
    &validation,
)…
```

`Validation::new` inserts `"exp"` into a fresh `required_spec_claims` HashSet and
allocates an `algorithms` Vec; `DecodingKey::from_secret` copies the secret into
a new Vec. `generate_token` did the same with `EncodingKey::from_secret`. All
three are now built once in `new()` and stored as fields:

```rust
pub struct WopiTokenService {
    encoding_key: EncodingKey,
    decoding_key: DecodingKey,
    validation: Validation,
    token_ttl_secs: i64,
}
```

The `secret` field is dropped — nothing else read it. **16 → 12 allocs/op** on
validate (the remaining 12 are the JWT crate's own base64/JSON claim
deserialization, paid by both arms). The four removed are exactly the
`Validation` HashSet + its `"exp"` String + the `algorithms` Vec + the
`DecodingKey` secret-copy. Existing `wopi_token_service` unit tests
(generate→validate round-trip, wrong-secret reject, read-only) pin the behaviour.

## [V1]/[V2] vCard per-contact emit — FN, NOTE, and the REV stamp renderer

`contact_to_vcard` (`carddav_adapter.rs`) and its twin `generate_vcard`
(`contact_service.rs`) emit one vCard **per contact** in every CardDAV REPORT,
`addressbook-multiget`, and collection PROPFIND that requests `address-data`
(i.e. every real DAVx5 / Apple Contacts / Thunderbird sync). Three per-contact
allocations:

1. **FN fallback** (`full_name` absent) built the mandatory `FN` from the
   name parts and copied the trimmed slice into a second owned `String`:
   ```rust
   let fn_name = format!("{} {}", first, last).trim().to_string();
   ```
   The `.to_string()` is redundant — `write!(vcard, "FN:{}\r\n", fn_name.trim())`
   writes the borrowed slice straight into the buffer. (The `format!` is kept:
   trimming *across* the join is subtle, and this arm is a fallback; dropping the
   copy is the unambiguously byte-identical win.)

2. **NOTE** ran `notes.replace('\n', "\\n")` unconditionally — a full copy of the
   note even when it has no newline (the common case), then formatted into the
   buffer and dropped. Now guarded: a newline-free note writes its borrowed slice
   directly; only a genuine multi-line note pays the escaping copy.

3. **REV** ran chrono's `updated_at.format("%Y%m%dT%H%M%SZ")` — and §V2 shows
   that `DelayedFormat` **allocates 3×** (not the 0 first assumed) while running
   the strftime spec interpreter. The new `common::fmt::compact_ical_utc(buf,
   secs)` renders the compact iCal/vCard UTC form `YYYYMMDDTHHMMSSZ` into a
   16-byte **stack** buffer via the same `push2`/`push4` LUT the RFC-3339/2822
   renderers use, falling back to chrono for out-of-range seconds.

Isolated (§V2), the stamp renderer is **11.77× faster and 3 → 0 allocs**
(216.9 → 18.4 ns). Over the whole per-contact emit (§V1, a contact exercising all
three shapes) that is **9 → 4 allocs/op, 1.97× wall** (548.2 → 277.7 ns). Both
`updated_at` fields are `DateTime<Utc>`, so `compact_ical_utc(ts.timestamp())` is
byte-for-byte the chrono output; `common::fmt`'s existing chrono-parity sweep
(every 6h13m across 60 years) now covers `compact_ical_utc` too.

## [M4] trash row → DTO — move, don't clone

`row_to_item_dto` takes an **owned** `TrashResourceRow` (consumed, dropped at fn
end) yet cloned its `String` fields into the DTO — `path` and `name` on a folder
row, plus `blob_hash` and `name` on a file row — up to 200 rows per
`GET /api/trash/resources` page:

```rust
let path = row.path.clone().unwrap_or_default();
…
name: row.name.clone(),
…
let content_hash = row.blob_hash.clone().unwrap_or_default();
```

Because `row` is owned, each field can be **moved** (`row.path.unwrap_or_default()`,
`name: row.name`, `row.blob_hash.unwrap_or_default()`). This is precisely what the
sibling `favorites_handler` / `recent_handler` / `folder_handler` row mappers
already do (with explicit "move it instead of cloning" comments); the trash path
was simply missed. **10 → 7 allocs/op** on the file branch (the remaining 7 are
`id.to_string()`, the interned display fields, and the `File::compute_etag`
stand-in — all unavoidable).

## [M5] search cache key — stack-encode the UUID

`SearchUseCase::search`'s `create_cache_key` hashes the criteria + a `&str`
user id; the caller fed it `user_id.to_string()`:

```rust
let user_id_str = user_id.to_string();                       // heap, per request
let cache_key = Self::create_cache_key(&criteria, &user_id_str);
```

`Uuid::hyphenated().encode_lower(&mut [u8; 36])` writes the identical 36-char
lowercase form into a **stack** buffer, so the hasher sees the same bytes ⇒ the
same `u64` key — the equivalence gate asserts it — with no allocation. The fn's
own doc-comment already claimed "zero-allocation hashing"; this makes it true.
**1 → 0 allocs/op.**

## [M6] streaming PROPFIND per-child href — one reused buffer

The streaming folder PROPFIND is the single most-travelled WebDAV path (every
folder listing, every desktop-sync descent). Both the native
(`webdav_handler.rs`) and NextCloud (`nextcloud/webdav_handler.rs`) handlers
built each child's `href` with a fresh `format!` per row — 4 loops, each up to
`PROPFIND_BATCH_SIZE` (500) rows/page:

```rust
for file in batch.iter() {
    let href = format!("{}{}", base_href, utf8_percent_encode(&file.name, …));
    …
}
```

One `String` per child. A single buffer hoisted out of the loop and rebuilt in
place (`href.clear(); href.push_str(base); href.extend(encode(name));`) keeps
its capacity across the page — the CalDAV/CardDAV emitters already thread reused
`href`/`etag` buffers exactly this way. On a 64-child page: **192 → 3 allocs/op,
2.74× wall** (10.9 → 4.0 µs); the 3 remaining are the buffer's initial grows to
the widest href. The equivalence gate asserts the emitted href set is
byte-identical.

## [M7] NextCloud `extract_url_user` — keep the Cow

Every path-scoped NC DAV request (`/remote.php/dav/{files,uploads,trashbin}/
{user}/…`) cross-checks the URL `{user}` segment against the session's
`raw_username`. The extractor forced an owned `String`:

```rust
urlencoding::decode(user_seg).ok().map(|s| s.into_owned())
```

`urlencoding::decode` returns `Cow::Borrowed` for a username with no
percent-escapes (the overwhelming common case), so `.into_owned()` allocates a
`String` on every request for nothing. Returning the `Cow` and comparing
`url_user.as_ref() != session.raw_username.as_str()` is zero-alloc on the common
path; only a percent-encoded username owns. **1 → 0 allocs/op, 3.11× wall.**

## Not shipped — deferred to a later round

Surfaced during the Round-19 audit but not landed (each needs Postgres, a
schema/DTO change, or its own decision):

- **CardDAV vCard etag buffer (`carddav_adapter::write_contact_response`):** the
  quoted `getetag` allocates a `String` per contact; the CalDAV emitter threads a
  reused `&mut String` etag buffer across the page but CardDAV's
  `write_contacts_report_page` never got the equivalent. Wants the buffer threaded
  through `write_contact_response` / `write_collection_contact_page` — a
  multi-signature change, deferred to keep this round's diff per-item-local.
- **CardDAV whole-book GET buffer (`carddav_handler::handle_get`):** the
  `text/vcard` export accumulates into a `String::new()` (repeated grows) and
  each `contact_to_vcard` allocates a per-contact throwaway `String` copied into
  it. Wants a `write_vcard_into(&mut String, …)` variant so the per-contact
  String disappears — an API addition, deferred.
- **BDAY stamp (`%Y-%m-%d` / `%Y%m%d`):** a `NaiveDate` date-only analogue of
  `compact_ical_utc`; only fires for contacts-with-birthday, so lower-priority
  than REV (every contact). A `compact_date` helper is the natural follow-up.
- **Search `suggest` DTO over-build (`search_service::suggest_with_perms`):**
  builds a full `FileDto`/`FolderDto` per candidate (≤20) on every keystroke only
  to copy out 5 fields — `size_formatted`/`content_hash`/`etag` are computed and
  dropped. Wants the fields pulled off the entity directly; deferred pending a
  small helper to avoid duplicating the display classifiers.
- **`grant_handler` shared-with-me deep clone (needs Postgres to bench the full
  path):** each shared item does `resource_id.to_string()` to key a map and a
  full DTO `.clone().without_hierarchy_info()`; a `remove`-and-move is valid only
  if summaries hold unique resource ids — verify before applying.

## Environment / methodology

- `cargo run --release --features bench --example bench_round19_micro` —
  counting global allocator, no Postgres. Tunable: `BENCH_ITERS` (200000; §M6
  uses a smaller default as each op is a whole 64-child page).
- Each section is BEFORE (verbatim replica of the shipped-before shape) vs AFTER
  (the shipped function itself where reachable — `common::fmt::compact_ical_utc`,
  `push_upper` — else a verbatim replica of the shipped-after shape), with a
  byte/-value equivalence gate; the shipped source now matches each AFTER arm.
- Roll-back rule encoded per section: the harness `std::process::exit(1)`s with
  `GATE FAIL … rollback` if an AFTER arm fails to reduce allocations (§M1, M2, V1,
  M4, M5, M6, M7) or, for the CPU-only §V2 stamp, fails to beat BEFORE by ≥2×
  wall. All eight sections pass.
