# Round 22 — hot-GET HeaderMap borrow, native-WebDAV & CalDAV etag borrowed quotes, FileDto content_hash move, CalendarEvent stamp, ShareItemType case-fold

Benchmark-gated, same rule as ROUND2–21: every change ships with a BEFORE/AFTER
benchmark and a byte/-value equivalence gate; an AFTER that doesn't beat its
BEFORE is rolled back (never applied). The roll-back rule is encoded directly in
the harness — a `GATE FAIL … rollback` non-zero exit if an AFTER arm fails to
reduce allocations — so a regression fails CI rather than shipping.

This round drains the two biggest items the ROUND21 audit explicitly deferred —
the hot-GET-handler `HeaderMap` extractor clone and the two DAV etag emitters the
borrowed-pre-escaped-quote sweep never reached (native WebDAV + CalDAV) — plus
the `FileDto::from` `content_hash` clone the ROUND19/20 move-not-clone sweep
missed (it is computed *before* `into_parts()`), and two low-heat strftime /
case-fold cuts.

Reproduce:

```
cargo run --release --features bench --example bench_round22_micro
```

All arms are **no-Postgres** (release-profile counting-allocator example).

## Summary

| # | change | key metric | before → after |
|--:|---|---|---|
| **H1** | The hot GET handlers (`get_thumbnail`, `download_file`, `list_files_query`, `list_photos`, NextCloud `preview`, public-share `download`/`access`) took axum's `HeaderMap` extractor, whose `FromRequestParts` impl does `parts.headers.clone()` — an owned clone of the **whole** request header table — purely to read 1–3 headers (`If-None-Match` / `Accept` / `Range` / unlock cookie). Now they take `req: Request` last and read `req.headers()` by borrow (the ROUND14 §A4 middleware pattern, finally propagated to the handlers). The 3 wrapper/`_impl` file handlers pass `req.headers()` into an `_impl` that now takes `&HeaderMap` (`+ use<>` on the return so the 2024-edition `impl Trait` capture doesn't tie the owned `Response` to the borrow). | realistic 13-header req | **2 → 0 allocs/op · 9.95× wall** |
| **W1** | `webdav_adapter::write_etag_quoted` — the etag emitter for **every** native `/webdav/` PROPFIND row (per file AND per folder, up to `PROPFIND_BATCH_SIZE`=500/page — the most-travelled DAV path) — built a sized `"{etag}"` `String` then wrote it auto-escaped; `quick_xml` escapes the `"` → `&quot;`, re-allocating an owned `Cow`. Now emits the two quotes as borrowed pre-escaped `&quot;` text events around the escaped body (the ROUND20 §C1 / ROUND21 §R4 pattern the native adapter never got). Byte-identical for any etag. | per PROPFIND row | **3 → 0 allocs/op · 1.59× wall** |
| **C1** | The CalDAV `getetag` emit — per event of every calendar-query/multiget/sync REPORT + depth-1 collection PROPFIND (the DAVx5/Apple/Thunderbird sync path), and per calendar of the home-set PROPFIND — still escaped a `"…"` value: the event sites paid the escape `Cow` over the ROUND14 reused buffer (1 alloc/event); the two calendar sites `format!`-ed as well (2 allocs). All **five** sites now route through a new `write_quoted_etag` helper (the CardDAV twin), and the now-dead `etag: &mut String` buffer threaded through `write_event_response`/`write_event_standard_props`/`write_event_requested_props` + the two page buffers are dropped. | per event row | **2 → 0 allocs/op · 1.63× wall** |
| **D1** | `FileDto::from` computed `content_hash = file.content_hash().to_string()` (a clone of `blob_hash`) and then `into_parts()` **moved** that same `blob_hash` into `parts.blob_hash`, which was dropped unused in the `Self { … }` ctor. The ROUND19/20 move-not-clone sweep fixed id/name/path/folder_id but missed this one because the etag/hash are read *before* `into_parts()`. Now `content_hash: parts.blob_hash` reuses the moved `String`; `etag` still computes first from the live entity. Runs **per file row on every listing** (folder browse, streaming PROPFIND, search/favorites/recent hydration). | per file row | **1 → 0 allocs/op · 2.91× wall** |
| **E1** | `CalendarEvent::update_time_range` / `update_all_day` stamped **timed** DTSTART/DTEND via `format!("{}", t.format("%Y%m%dT%H%M%SZ"))` — chrono's strftime `DelayedFormat` interpreter. Now stack-renders via the shipped `fmt::compact_ical_utc` and passes the `&str` straight to `update_ical_property`, with the chrono `format!` kept as the out-of-range fallback and the all-day `%Y%m%d` form untouched. Per event-edit PUT. | per timed stamp | **4 → 0 allocs/op · 14.49× wall** |
| **S1** | `ShareItemType::try_from` matched `s.to_lowercase().as_str()` — a throwaway Unicode-lowercased `String` — against the two ASCII literals `"file"`/`"folder"`. Now `s.eq_ignore_ascii_case("file")` / `("folder")`: byte-identical acceptance for the ASCII targets, no allocation. | per parse | **1 → 0 allocs/op · 7.81× wall** |

> Allocs/op is the deterministic primary gate (identical run to run). Wall
> figures are single-shot and noise-bounded. Every section carries a
> byte/-value equivalence gate; the shipped source now matches each AFTER arm.

## [H1] Hot GET handler `HeaderMap` extractor → `Request` + borrow

axum 0.8's `impl FromRequestParts for HeaderMap` is literally
`Ok(parts.headers.clone())` — cloning the whole request header table (its
`entries` + `indices` backing vectors; the counting allocator measures exactly
2 allocs on a realistic 13-header browser request). The handlers below read only
1–3 headers out of it, so the clone is pure waste — the exact cost
`middleware/auth.rs` removed in ROUND14 §A4 (`request.headers().get(…)` by
borrow) but which was never propagated to the handlers.

The fix takes `req: Request` as the **last** extractor (all the others —
`State`, `AuthUser`, `Path`, `Query` — are `FromRequestParts`, so they coexist
with a single trailing `FromRequest`), and reads `req.headers()` by borrow:

- **Standalone handlers** (`list_photos`, NC `preview`, share `download`/`access`):
  swap `headers: HeaderMap` for `req: Request` and read `req.headers().get(…)`
  at the (single) use site.
- **Wrapper/`_impl` handlers** (`get_thumbnail`, `download_file`,
  `list_files_query`): the wrapper takes `req: Request` and passes
  `req.headers()` into an `_impl` whose param becomes `headers: &HeaderMap`. The
  `_impl` return type gets `+ use<>` so the 2024-edition `impl Trait` lifetime
  capture doesn't tie the (owned) `Response` output to the header borrow — the
  future still borrows the headers during its inline `.await`, but the response
  it yields captures nothing.

Byte-identical: every call site reads the same header by `.get()`. The
`openapi_spec_is_valid_and_has_expected_structure` test confirms the
utoipa-annotated handlers still emit a valid spec after the signature change.

NextCloud `avatar` (dual caller `handle_dav_avatar` → `handle_avatar` + dual
route) and the share-management handlers (`create`/`update`/… take a `Json`
body, so no second `Request` extractor is possible) were left for a dedicated
pass — see *Not shipped*.

## [W1] Native WebDAV `getetag` — borrowed pre-escaped quotes

`write_etag_quoted` is the single helper behind all four native PROPFIND etag
sites (`webdav_adapter.rs:857/935/1004/1089` — file + folder, allprop + named).
It built a `String::with_capacity(etag.len()+2)` `"{etag}"` and wrote it via
`BytesText::new`, which escapes the `"` → `&quot;` and re-allocates an owned
`Cow`. Now (the ROUND20 §C1 / ROUND21 §R4 shape):

```rust
xml_writer.write_event(Event::Text(BytesText::from_escaped("&quot;")))?; // borrowed
xml_writer.write_event(Event::Text(BytesText::new(etag)))?;             // escaped body
xml_writer.write_event(Event::Text(BytesText::from_escaped("&quot;")))?;
```

`escape` maps `"`→`&quot;` per char, so `&quot;{escape(etag)}&quot;` is
byte-identical to escaping `"{etag}"` for **any** etag (the equivalence gate
asserts it, including an etag carrying `&`/`<`/`"`). One helper body fixes all
four call sites — 0 allocs/row on the hottest native-WebDAV path.

## [C1] CalDAV `getetag` — shared `write_quoted_etag` helper (5 sites)

The CalDAV adapter was the last DAV emitter still escaping a quoted etag value.
A new file-local `write_quoted_etag` (identical to the shipped CardDAV twin)
replaces the manual quote-and-escape at all five sites:

- `write_event_standard_props` / `write_event_requested_props` /
  `write_collection_event_page` — **per event bundle** (the reused ROUND14
  buffer was already amortized, so the remaining cost was the escape `Cow`;
  1 → 0 alloc/event).
- `write_calendar_standard_props` / `write_calendar_requested_props` — **per
  calendar**, which additionally `format!`-ed the value (2 → 0).

The etag bodies are bare `Uuid`s (`anchor.id` / `calendar.id`), so
`BytesText::new(id)` is itself a borrow (0 allocs). With the emit no longer
needing a scratch `String`, the `etag: &mut String` buffer threaded through
`write_event_response` → `write_event_standard_props` /
`write_event_requested_props` and the two per-page `String::new()` buffers were
removed. The 34 caldav-adapter unit tests (PROPFIND/REPORT output) pass
unchanged.

## [D1] `FileDto::from` — reuse the moved `blob_hash`, don't clone it

The per-row DTO builder computed the ETag and the content hash from the live
entity, then consumed it:

```rust
let etag = file.etag();
let content_hash = file.content_hash().to_string();   // clone of self.blob_hash
let parts = file.into_parts();                         // MOVES self.blob_hash → parts.blob_hash
// … Self { …, content_hash, etag, … }                // parts.blob_hash dropped unused
```

`etag` genuinely must run against the live entity (it borrows `blob_hash` +
`modified_at`), but `content_hash` is just the raw hash — and `into_parts()`
already hands it over by ownership. Now `content_hash: parts.blob_hash` reuses
that `String`; the getter clone (one 64-byte hex `String` per row) is gone. This
is the file-side twin of the fields `FolderDto::from` already moves, on the
single most-travelled API path. Byte-identical: `parts.blob_hash` **is** the
`String` the getter cloned.

## [E1] `CalendarEvent` timed DTSTART/DTEND — `compact_ical_utc` stack render

The timed branches of `update_time_range` / `update_all_day` stamped
`format!("{}", t.format("%Y%m%dT%H%M%SZ"))`, running chrono's strftime
interpreter (4 allocs measured). `fmt::compact_ical_utc` already renders exactly
`YYYYMMDDTHHMMSSZ` on the stack (the ROUND19 §V2 helper), and the property
setter takes a `&str`, so the render is passed straight through with no owned
`String`:

```rust
let start_str: &str = if self.all_day {
    start_owned = format!("{}T000000Z", start_time.format("%Y%m%d")); &start_owned
} else if let Some(s) = fmt::compact_ical_utc(&mut sbuf, start_time.timestamp()) {
    s                                          // 0 allocs, the common case
} else {
    start_owned = format!("{}", start_time.format("%Y%m%dT%H%M%SZ")); &start_owned  // fallback
};
```

The all-day `%Y%m%d` + literal-suffix form is unchanged (no existing
no-separator helper covers it — see *Not shipped*). The 20 calendar_event unit
tests (iCal round-trip, exception handling) pass unchanged.

## [S1] `ShareItemType::try_from` — `eq_ignore_ascii_case`

`match s.to_lowercase().as_str()` allocated a Unicode-lowercased `String` per
call only to compare against `"file"`/`"folder"`. `eq_ignore_ascii_case` folds
only ASCII A–Z — but the targets are pure ASCII, and any input whose
`to_lowercase()` equals `"file"`/`"folder"` is by definition an ASCII case
variant of it, so acceptance is byte-identical (the gate checks mixed-case +
invalid inputs). 0 allocs.

## Not shipped — deferred to a later round

Surfaced by the Round-22 audit (three parallel sub-audits across the HTTP, DAV
and application/parse layers), verified against current source, but held back —
each needs a signature/API decision or a gate the deterministic alloc-counter
can't provide:

- **`list_files_query` `Query<HashMap<String,String>>` → typed `Query<…>`**: the
  listing reads only `folder_id`, so a `struct ListFilesQuery { folder_id:
  Option<String> }` drops the `HashMap` table + the `"folder_id"` key `String`
  (~3 → 1 allocs). Byte-identical for the frontend's actual usage, but a
  **malformed** `?folder_id=a&folder_id=b` diverges (HashMap last-wins vs serde
  field-decode), so it wants its own byte-identity proof before shipping — the
  H1 half of this handler is unimpeachable and shipped alone.
- **NextCloud `avatar` HeaderMap clone**: `handle_avatar` has two callers
  (`handle_dav_avatar` + a direct route), so the `Request` conversion is a
  dual-signature change, not the clean leaf swap the other H1 handlers were.
  Low frequency (avatars revalidate hourly).
- **Share-management HeaderMap clones** (`create`/`update`/`verify`/… at
  `share_handler.rs:583+`): these take a `Json` body (a `FromRequest` body
  extractor), so a second `Request` extractor is impossible — they need a
  different borrow strategy. Lower frequency than the public download/access
  path shipped here.
- **`update_all_day` / `update_time_range` all-day `%Y%m%d` stamp**: no
  no-separator date helper exists (`compact_ical_utc` is date+time,
  `compact_date` is `YYYY-MM-DD`); a `compact_date_basic` (`YYYYMMDD`) would
  close the remaining 2 all-day sites. Low heat.
- **`ContactService::generate_vcard` BDAY** (`contact_service.rs:342`) still uses
  `birthday.format("%Y%m%d")` on the contact write path — same missing
  `%Y%m%d` helper as above; the per-contact *read* twin was already fixed
  (ROUND21 §R5). Low heat.
- **`extract_webdav_path(req.uri())`** (`webdav_handler.rs:507`): a per-PROPFIND
  percent-decode + `String`, but byte-identity is **unproven** — the code
  comment states the `path` parameter carries a home-folder prefix that is
  wrong for WebDAV hrefs, directly contradicting ROUND21's "stale comment"
  note. Needs a dedicated href-equivalence proof, not a perf banner.

## Environment / methodology

- `cargo run --release --features bench --example bench_round22_micro` —
  counting global allocator, no Postgres. Tunable (env): `BENCH_ITERS` (200000).
- Each section is BEFORE (verbatim replica of the shipped-before shape) vs AFTER
  (verbatim replica of the shipped-after shape, which the source is then made to
  match), with a byte/-value equivalence gate; the shipped source now matches
  each AFTER arm.
- Roll-back rule encoded per section: the harness `std::process::exit(1)`s with
  `GATE FAIL … rollback` if an AFTER arm fails to reduce allocations.
- Verified beyond the bench: `cargo clippy --features bench --all-targets -D
  warnings` clean, `cargo fmt --all --check` clean, and `cargo test --lib
  --features bench` = **529 passed / 0 failed** (incl. the OpenAPI-spec-validity
  test that guards the H1 utoipa-handler signature change).
```
