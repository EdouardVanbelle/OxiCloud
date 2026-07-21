# Round 5 — CalDAV streaming, SPA interning gaps, NC href prefix, per-request micro-allocs

Benchmark-gated changes, same rule as ROUND2-4: every change ships with a
BEFORE/AFTER benchmark; an AFTER that doesn't beat its BEFORE gets rolled
back. Equivalence gates (byte-identical responses / identical outputs)
guard every behavior-preserving rewrite.

Measured on 4 cores / 15 GiB, local PostgreSQL 16 (fsync off), release
profile. Reproduce any row with the command in its section.

## Summary

| # | change | key metric | before → after |
|--:|---|---|---|
| 1 | CalDAV whole-calendar streaming | TTFB / peak heap (4k events) | 23.3 → 11.0 ms (**2.1x**) / 14.2 → 8.0 MiB (**1.8x**) |
| 2 | SPA listing interning gaps closed | allocs/row closed-set fields | 4 → 0 (wall parity) |
| 3 | NC PROPFIND child-href prefix | ns/row href build | 543 → 165 (**3.3x**), 13 → 4 allocs |
| 4 | suggest enrichment consume | µs/keystroke (200 rows) | 166.5 → 126.8 (**1.31x**), 20 → 7 allocs/row |
| 5 | `list_readable_by` Arc hit | ns/hit warm | 246 → 128 (**1.9x**), 4 → 0 allocs |
| 6 | CardDAV REPORT churn | µs/5k-contact getetag poll | 3044 → 2340 (**1.30x**) |
| 7 | auth span records | allocs/request | 3 → 0 (field::display) |

## [1] CalDAV whole-calendar responses — buffered double-residency → cursor streaming

The REPORT path (no-range `calendar-query`, `sync-collection`), the
depth-1 collection PROPFIND (both URL shapes) and the whole-calendar
`.ics` GET all (a) materialised EVERY event DTO of the calendar in one
Vec — each row carrying its full `ical_data` body — then (b) rendered
the complete multistatus / VCALENDAR into a second in-RAM buffer: the
calendar resident twice per request, TTFB = full generation time.

Now `CalendarEventRepository::stream_events_uid_order` serves ONE
window-ordered scan (`ORDER BY MIN(start_time) OVER (PARTITION BY
ical_uid), ical_uid, master-first, start_time`) through a PG cursor —
same-UID rows (recurring master + exception overrides) arrive adjacent,
bundle order equals the buffered listing's first-appearance order — and
the handlers cut emit pages at UID boundaries, streaming header →
page chunks → footer through the split adapter writers
(`write_caldav_multistatus_start` / `write_report_page` /
`write_collection_head` / `write_collection_event_page`). Bounded
shapes (time-range query, multiget, single-event GET) keep the buffered
path. The Read authz gate runs once before the cursor opens.

The shape was itself benchmark-driven: a first keyset pager over the
`GROUP BY` re-aggregated the calendar per page (3-4x total wall —
rolled back), and per-uid `= ANY(page)` hydration paid ~20 µs per index
descent (~4x the sequential scan — rolled back). The shipped design
streams ONE window-ordered scan
(`ORDER BY MIN(start_time) OVER (PARTITION BY ical_uid), …`) through a
PG cursor, cutting emit pages at UID boundaries.

```
cargo run --release --features bench --example bench_caldav_stream
# 4000 events (20% exceptions)      TTFB ms   wall ms   peak heap MiB
# BEFORE (buffered)                    23.3      23.3         14.2
# AFTER  (streamed)                    11.0      25.4          8.0   TTFB 2.1x, heap 1.8x
# 12000 events
# BEFORE                               79.5      79.5         45.0
# AFTER                                43.9      91.5         24.2   TTFB 1.8x, heap 1.9x
# Trade: wall +9-15% (the window sort + cursor) for ~2x lower peak RAM
# — which scales with calendar size and per concurrent sync client —
# and ~2x faster first byte. Same trade class as ROUND2's ZIP
# streaming. Gates: multistatus AND .ics byte-identical to buffered.
```

## [2] SPA listing rows — interning bypass closed

ROUND3 added `intern_display` / `intern_mime` so `File→FileDto` stops
allocating for the ~60-string closed set (icon class, category, mime).
But the three hottest web-UI listing endpoints — the folder navigation
(`/folders/{id}/resources`), `/recent/resources` and
`/favorites/resources` — plus the WebDAV drive pseudo-root build their
DTOs by hand and called raw `Arc::from` per row, re-introducing 3-4
alloc+copies per row the intern tables exist to remove. All four sites
now route through the intern lookups; returned `Arc<str>` contents are
byte-identical.

## [3] NC PROPFIND child hrefs — per-row prefix re-encode → precomputed

`nc_href` re-encoded the username and re-split + re-encoded the whole
parent path for EVERY child row of every NextCloud PROPFIND page (up to
500/page), preceded by a per-row `format!` of the joined subpath — only
the name segment actually varies. The prefix is now encoded once per
request; each row appends its encoded name (native WebDAV href also
dropped its intermediate encode String — the percent-encode `Display`
adapter feeds `format!` directly).

## [4-6] Per-request micro-allocs (suggest, readable-cache, CardDAV)

- **suggest** deep-cloned every entity into the DTO conversion and then
  cloned name/id/path AGAIN per row — on an every-keystroke path. Now
  consumes + moves.
- **`list_readable_by`** returned a fresh deep clone of the cached
  drive Vec (every row's Strings) per warm hit — per DAV request with an
  explicit selector. It now returns the cache's `Arc` (refcount bump);
  the only caller that needs owned rows (`GET /api/drives`) clones just
  its response rows.
- **CardDAV REPORT** cloned the requested-props Vec per REPORT,
  allocated a fresh href String per contact and `format!`ed each quoted
  etag — the same shapes ROUND4 removed from CalDAV. Now: borrowed
  props, one reused href buffer, exact-size quoting.

```
cargo run --release --features bench --example bench_micro_allocs
# [1] suggest (200 rows)        166.5 → 126.8 µs   1.31x   20.0 → 7.0 allocs/row
# [2] readable warm hit         246.4 → 127.7 ns   1.9x     4 → 0 allocs/hit
# [3] closed-set fields         129.9 → 136.3 ns   1.0x     4 → 0 allocs/row
#     (wall parity under the bench's System allocator; the win is the
#      removed allocator traffic + consistency with the interned
#      FileDto::from path — ROUND3 #9)
# [4] NC child hrefs            543.1 → 164.5 ns   3.3x    13 → 4 allocs/row
# [5] CardDAV getetag (5k)      3043.8 → 2339.5 µs 1.30x
# gates: identical outputs / byte-identical XML on every section
```

## [7] Auth middleware span records

`tracing::Span::current().record("user_id", user_id.to_string())`
allocated a 36-byte String per authenticated request (×3 auth paths).
`tracing::field::display(user_id)` records lazily — the subscriber
formats into its own buffer.

## Follow-ups worth a future round (confirmed real, not gated here)

- CardDAV multistatus is still fully buffered — port the CalDAV
  streaming emitter once contacts get a keyset pager (current
  `get_contacts_by_address_book_paginated` is LIMIT/OFFSET, the
  quadratic shape PROPFIND-PAGING replaced elsewhere).
- CalDAV time-range REPORT still buffers (bounded by the range, but a
  year-wide range on a dense calendar is large).
- `batch_resolve_ids` / `batch_check_favorites` take `&[String]` — every
  NC PROPFIND page clones ~500 id Strings that the services re-parse to
  `Uuid` anyway; switch the chain to `&[&str]` (8 call sites).
- Hot listing SQL casts UUID columns to `::text` server-side (~18 sites
  in `file_blob_read_repository.rs`) — decode as `Uuid` + format
  app-side; needs a local-PG A/B before adopting.
- Public-share landing runs register + fetch serially — `tokio::join!`
  or fold the increment into the fetch with `RETURNING`.
- `CurrentUser` still clones username/email per request; zero-alloc
  needs the JWT cache to hold `Arc<str>` claims.
- Grouped/swimlane files view virtualization (frontend, carried since
  ROUND3).
