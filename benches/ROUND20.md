# Round 20 — parse-path HashMap purge, owned-DTO moves, Result-collect pre-size, NC etag/favorites emit

Benchmark-gated, same rule as ROUND2–19: every change ships with a BEFORE/AFTER
benchmark and a byte/-value equivalence gate; an AFTER that doesn't beat its
BEFORE is rolled back (never applied). The roll-back rule is encoded directly in
the harness — a `GATE FAIL … rollback` non-zero exit if an AFTER arm fails to
reduce allocations — so a regression fails CI rather than shipping.

This round drains three seams the earlier passes left: the **inbound parse
paths** (CalDAV iCal, CardDAV vCard) that rounds 4–19 optimized on the *emit*
side but not on ingest; three **owned-entity → DTO conversions** that the
`into_parts` move-not-clone rounds skipped (`User`, `Calendar`, `AddressBook`);
and a **stdlib footgun** — `collect::<Result<Vec<_>, _>>()` never pre-sizes — on
the file-listing repositories. Plus two NextCloud DAV emit micro-cuts the M4/M6
row passes didn't reach.

Reproduce:

```
cargo run --release --features bench --example bench_round20_micro
```

All arms are **no-Postgres** (release-profile counting-allocator example).

## Summary

| # | change | key metric | before → after |
|--:|---|---|---|
| **A1** | `CalendarEvent::prop_with_params` built a throwaway `HashMap<String, Vec<String>>` (uppercased keys + cloned value Vecs) per DTSTART/DTEND/RECURRENCE-ID on **every CalDAV PUT / iCal import**, though all 5 production call sites only read `.get("VALUE")` (all-day detect) or discarded the map. Now `prop_value_and_is_date` scans `prop.params` directly for a case-insensitive `VALUE=DATE`. | per timed event | **6 → 2 allocs/op · 4.15× wall** (168.8 → 40.7 ns) |
| **A2** | `UserDto::from(User)` took the `User` **by value** yet cloned every field through its accessors — including `image` (a data URI up to **512 KiB**) and `ui_preferences` (a full `serde_json::Value` tree) — on **every `/api/auth/me`** and admin user listing. Now `User::into_parts()` moves the owned fields (the treatment File/Folder/Contact already had). | 48 KiB avatar user | **27 → 14 allocs/op · 2.14× wall** (image memcpy + JSON deep-clone gone) |
| **A3** | `ContactService::parse_vcard` collected `vcard_data.lines()` into a `Vec` it only iterated, and ran `line.to_ascii_uppercase()` — a full per-line `String` copy — per EMAIL/TEL/ADR line just to `.contains` a `TYPE=` token, on **every CardDAV PUT / vCard import**. Now iterates `lines()` directly and matches with the allocation-free `common::text::ascii_ci_contains` (the CalDAV parse path already used this shape). | 2 email / 1 tel / 1 adr | **8 → 1 allocs/op · 1.67× wall** (444.8 → 266.1 ns) |
| **A4** | `CalendarDto::from` / `AddressBookDto::from` consumed the entity yet cloned `name`/`description`/`color` and (calendars) the whole `custom_properties` `HashMap<String,String>`, on **every CalDAV/CardDAV discovery listing**. Now `Calendar`/`AddressBook` grow `into_parts()` and move them. | calendar + 2 props | **18 → 10 allocs/op · 1.78× wall** |
| **I1** | The file-listing repositories map rows with `.collect::<Result<Vec<T>, E>>()`, whose `Result`-shunt reports `size_hint().0 == 0` — so the `Vec` grows **from capacity 0** with ~⌈log₂N⌉ reallocations, memcpy-ing the accumulated `File`-sized rows each grow. Now `Vec::with_capacity(rows.len())` + push with `?` (the pattern `list_media_files` already used). | 500-row listing | **8 → 1 allocs/op** (container reallocs 8 → 0) |
| **I4** | `encrypted_blob_backend::plaintext_stream` `.collect()`ed every emit-slice into a `Vec` before `stream::iter` — an eager container of ⌈len/64 KiB⌉ entries per **encrypted-blob read**. Now hands the lazy `map` iterator to `stream::iter` directly (same slice sequence). | 4 MiB → 64 slices | **2 → 1 allocs/op · 42.85× wall** (1732.8 → 40.4 ns) |
| **C1** | NC `write_etag_element` built a `"…"`-quoted `String` then wrote it auto-escaped — `quick_xml` escapes the `"` → `&quot;`, re-allocating an owned `Cow`. Called **per file AND per folder row** of the NC streaming PROPFIND (the hottest DAV emit path), plus every favorites/search REPORT row and trashed item. Now emits the two quotes as **borrowed pre-escaped** `&quot;` text events around the escaped body. | per PROPFIND row | **3 → 0 allocs/op · 1.71× wall** (137.9 → 80.5 ns) |
| **C3** | The NC favorites REPORT (`oc:filter-files`) hydrated `files`/`folders` by `file_map.get(&id).clone()` — cloning the **whole** `FileDto`/`FolderDto` out of maps that are dropped at fn end. Now `map.remove(&id)` moves them (item ids are unique per user; favorites order preserved — the round-19 M4 move-not-clone pattern applied to a path it missed). | 20 favorites | **302 → 162 allocs/op · 1.35× wall** (~7 allocs/favorite) |

> Allocs/op is the deterministic primary gate (identical run to run). Wall
> figures are single-shot and noise-bounded. Every section carries a
> byte/-value equivalence gate; the shipped source now matches each AFTER arm.

## [A1] CalendarEvent iCal parse — drop the per-property parameter HashMap

`from_ical` and `update_ical_data` parse a VEVENT once, then read DTSTART, DTEND
and RECURRENCE-ID via `prop_with_params`, which built a full
`HashMap<String, Vec<String>>` per property:

```rust
let mut params: HashMap<String, Vec<String>> = HashMap::new();
if let Some(param_list) = &prop.params {
    for (name, values) in param_list {
        params.insert(name.to_ascii_uppercase(), values.clone());   // upper key + value clone
    }
}
Some((trimmed.to_string(), params))
```

Every production caller only ever asked the map one question — *does it carry
`VALUE=DATE`?* (the all-day / date-only marker) — and the two DTEND sites
discarded the map outright (`_dtend_params`, `_params`). The new
`prop_value_and_is_date` answers exactly that, scanning `prop.params` directly:

```rust
let is_date = prop.params.as_ref()
    .and_then(|list| list.iter().rev().find(|(n, _)| n.eq_ignore_ascii_case("VALUE")))
    .map(|(_, vs)| vs.iter().any(|v| v.eq_ignore_ascii_case("DATE")))
    .unwrap_or(false);
```

`.rev().find(...)` reproduces the old map's last-insert-wins semantics for a
(pathological) duplicate-`VALUE` property, so the flag is byte-identical; DTEND
now uses the plain `prop_value`. `prop_with_params` is retained behind
`#[cfg(test)]` for its existing test wrapper. On a timed event (DTSTART+DTEND,
each with a `TZID`): **6 → 2 allocs/op, 4.15× wall** — the 2 remaining are the
DTSTART/DTEND value strings the callers need owned.

## [A2] UserDto::from — move the 512 KiB image + JSON, don't clone

`UserDto::from` consumes an owned `User` yet cloned every field through the
borrowing accessors. Two of them are large: `image` is "a data URI of up to
512 KiB" (the entity's own comment) and `ui_preferences` is a
`serde_json::Value` tree — both deep-cloned on **every `/api/auth/me`** (session
bootstrap on every app load, and after each profile edit) and once per user in
admin listings. `User` was the one core entity without `into_parts`; adding it
(exhaustive-destructure, compiler-checked) lets the conversion move:

```rust
let role = format!("{}", user.role());
let can_edit_image = !user.is_oidc_user();   // derived flags read before the move
let p = user.into_parts();
… image: p.image, ui_preferences: p.ui_preferences,
  auth_provider: p.oidc_provider.unwrap_or_else(|| "local".to_string()), …
```

**27 → 14 allocs/op, 2.14× wall** — the `image` memcpy + `String` alloc, the
`ui_preferences` deep-clone, and 5 small field clones are gone; the OIDC-user
`auth_provider` also stops re-allocating (moves the provider `String`). The DTO
is byte-identical.

## [A3] parse_vcard — allocation-free `TYPE=` routing

`parse_vcard` (every CardDAV PUT / bulk import) collected the body into
`Vec<&str>` it only iterated, and per EMAIL/TEL/ADR line ran
`line.to_ascii_uppercase()` — a whole-line copy — purely to `.contains("TYPE=…")`.
This is the exact allocation the CalDAV parse path already killed with
`starts_with_ci`/`find_ci`; `ascii_ci_contains` was promoted from
`search_service` to the shared `common::text` module (DRY) and both callers now
use it. **8 → 1 allocs/op, 1.67× wall** for a 2-email/1-phone/1-address card
(the remaining alloc is the result Vec both arms build).

## [A4] Calendar/AddressBook DTO — finish the into_parts family

`CalendarDto::from` / `AddressBookDto::from` consumed the entity but cloned
`name`/`description`/`color` and — for calendars — the whole
`custom_properties` `HashMap<String,String>`, on every CalDAV/CardDAV discovery
listing (DAVx5/Apple poll these repeatedly). Both entities grew `into_parts()`
and the conversions move. **18 → 10 allocs/op, 1.78× wall** (the HashMap clone +
3 string clones gone; the two `Uuid::to_string`s remain).

## [I1] Result-collect never pre-sizes — the file-listing repositories

`collect::<Result<Vec<T>, E>>()` collects through a `Result` shunt whose
`size_hint().0` is `0` (any element may short-circuit the collect), so `Vec`'s
`extend` reserves nothing and the container grows **from capacity 0** — ~⌈log₂N⌉
reallocations, each memcpy-ing the accumulated `File` rows (≈120 B apiece). The
bench isolates the container behaviour on 500 File-sized rows: **8 container
reallocations → 0** (one `with_capacity` alloc). Applied to the four
`file_blob_read_repository` listing/paging/subtree/by-ids mappers (the hottest
paths — folder browse, PROPFIND, search, favorites/ACL hydration); the fix is
the loop `list_media_files` already used:

```rust
let mut files = Vec::with_capacity(rows.len());
for (id, name, …) in rows {
    files.push(Self::row_to_file(id, name, …).map_err(…)?);
}
Ok(files)
```

`?` short-circuits on the first row error exactly as the `Result`-collect did —
byte-identical behaviour and error message.

## [I4] plaintext_stream — lazy emit iterator

The encrypted backend's `plaintext_stream` `.collect()`ed a
`Vec<Result<Bytes>>` of ⌈len/64 KiB⌉ zero-copy slices before handing it to
`stream::iter` — an eager container built per encrypted read (a legacy
whole-file blob → thousands of entries). The `move` closure owns the refcounted
`Bytes`, so the `map` iterator is `Send + 'static` and can be streamed lazily.
**2 → 1 allocs/op, 42.85× wall** (the eager Vec build + fill is gone; each slice
is now produced on demand as the consumer polls, also cutting peak RAM).

## [C1] NC write_etag_element — borrowed pre-escaped quotes

`write_etag_element` is called per file **and** per folder row of the NC
streaming PROPFIND — the single most-travelled DAV emit path — plus every
favorites/search REPORT row and trashed item. It built a `"…"`-quoted `String`
and wrote it auto-escaped; `quick_xml` escapes a literal `"` to `&quot;`, so the
whole-string escape re-allocated an owned `Cow` (3 allocs total, measured). The
new form emits the two quotes as **borrowed** pre-escaped `&quot;` text events
around the escaped etag body:

```rust
xml.write_event(Event::Text(BytesText::from_escaped("&quot;")))?;   // borrowed, 0 alloc
xml.write_event(Event::Text(BytesText::new(etag)))?;                // escaped body
xml.write_event(Event::Text(BytesText::from_escaped("&quot;")))?;
```

The output is byte-identical to escaping `"{etag}"` as one string — the
equivalence gate asserts it, including an etag with `&`/`<`/`"`. **3 → 0
allocs/op, 1.71× wall.**

## [C3] favorites REPORT — move the DTO out of the map

`oc:filter-files` builds `file_map`/`folder_map` two lines before the hydrate
loop, uses them only to populate `files`/`folders` in favorites order, and drops
them at fn end — yet cloned the **whole** DTO out with `.get().clone()`. Since
`favorites.item_id` is unique per user, `.remove()` moves the DTO out with no
risk of dropping a needed duplicate and preserves order (the round-19 M4
pattern). **302 → 162 allocs/op** for a 20-favorite page — ~7 owned-String
allocs saved per favorite.

## Not shipped — deferred to a later round

Surfaced during the Round-20 audit, measured or confirmed, but held back to keep
this round's diff focused / because they need Postgres or a dependency decision:

- **NC `oc:id` per-row `String` (`format_oc_id`):** `format!("{:08}{instance}")`
  allocates one `String` per PROPFIND/REPORT/trashbin row. A `format_oc_id_into(&mut
  String, …)` buffer reused across the page (mirroring the M6 href buffer already
  threaded through those loops) makes it **1 → 0 allocs/row** — but it's a
  multi-signature change through `write_file_response`/`write_folder_response`,
  deferred to keep this round per-item-local.
- **NC trashbin PROPFIND per-item href + folder content_type:** the trashbin loop
  still `format!`s each `href` and `"httpd/unix-directory".to_string()`s the folder
  content-type per row — the M6 href-buffer + `Cow<'static, str>` fix that reached
  the files/folders loops but not trashbin.
- **I1 sibling listing paths:** the same `collect::<Result<Vec>>()` /
  `Vec::new()`+push shape lives in the CardDAV (`contact_pg_repository`,
  `contact_group_pg_repository`) and CalDAV (`calendar_event_pg_repository`,
  `calendar_pg_repository`) row mappers. Mechanically identical to the file-side
  fix shipped here; extend next (bulk address-book / calendar sync builds
  thousands of rows).
- **Contact JSONB columns decode through a throwaway `serde_json::Value`
  (`contact_pg_repository::row_to_contact`, needs Postgres to bench):**
  `row.get::<JsonValue>` builds a full `Value` tree per email/phone/address column
  before `from_value` walks and drops it. `sqlx::types::Json<Vec<Dto>>` runs
  `from_slice` on the raw JSONB — same Vec, no intermediate tree, tens of allocs
  saved per contact.
- **Dedup `settle_batch` clones chunk-hash `String`s for the SQL array bind
  (`dedup_service`, needs Postgres):** `batch.iter().map(|(h, _)| h.clone())` deep-
  clones each 64-char hash purely to `.bind()`, though `batch` outlives the query;
  `&[&str]` encodes to `text[]` identically — up to 32 fewer allocs per new-content
  batch (~4000 over a 1 GB upload).
- **Fast hasher for internal maps (cross-cutting, needs a dependency decision):**
  every `HashMap`/`HashSet` in the tree uses std SipHash. Trusted-key,
  built-per-request maps would benefit from a faster `BuildHasher` — the hottest
  are the NC PROPFIND per-row `favorite_ids.contains(&file.id)` /
  `nc_id_of` lookups, and the delta-upload `distinct_hashes` /
  `authorize_chunk_download` sets over up to ~40 000 client-supplied 64-char
  hashes. Two caveats keep it out of this round: it changes **no allocations** (so
  it can't use the alloc gate — only the noisy wall metric), and it needs a
  `Cargo.toml` dependency; the delta sets are **attacker-controlled**, so the
  replacement must stay DoS-resistant (`ahash`/`foldhash` with a random seed, not
  `FxHash`). Worth a dedicated, wall-gated evaluation.

## Environment / methodology

- `cargo run --release --features bench --example bench_round20_micro` —
  counting global allocator, no Postgres. Tunables (env): `BENCH_ITERS` (200000),
  `I1_ROWS` (500).
- Each section is BEFORE (verbatim replica of the shipped-before shape) vs AFTER
  (verbatim replica of the shipped-after shape, which the source is then made to
  match), with a byte/-value equivalence gate; the shipped source now matches
  each AFTER arm.
- Roll-back rule encoded per section: the harness `std::process::exit(1)`s with
  `GATE FAIL … rollback` if an AFTER arm fails to reduce allocations. All eight
  sections pass.
