# Round 18 — calendar-event in-place iCal edit, ResourceList incremental id-index

Benchmark-gated, same rule as ROUND2–17: every change ships with a
BEFORE/AFTER benchmark and an equivalence/safety gate; an AFTER that doesn't
beat its BEFORE is rolled back (never applied). The roll-back rule is encoded
directly into each harness — a `GATE FAIL … rollback` non-zero exit (Rust) or a
failing `expect(afterMs).toBeLessThan(beforeMs / K)` assertion (vitest) — so a
regression fails CI rather than shipping.

This round picks up two items carried on the **ROUND17 deferred list**:

- the **REST calendar-event edit** that re-`format!`'d the whole `ical_data`
  body once per changed property (backend, no Postgres — counting-allocator
  example), and
- **`ResourceList.itemIndexById`**, which re-scanned the whole accumulated list
  per infinite-scroll page (frontend, vitest-benchmarked).

Reproduce:

```
cargo run --release --features bench --example bench_round18_micro
cd frontend && npx vitest run src/lib/components/round18.bench.test.ts
```

## Summary

| # | change | key metric | before → after |
|--:|---|---|---|
| **C1** | `CalendarEvent::update_ical_property` / `remove_ical_property` rewrote the ENTIRE `ical_data` body with `format!("{}{}{}")` on every call and allocated **two** search needles per call (`\nNAME:` + the redundant `\r\nNAME:`). `calendar_storage_adapter::update_event` fans a multi-field edit out into one `update_ical_property` per changed field, so a full edit paid one full-body (up to ~11 KB) allocation **per property**. Now the body is mutated in place (`replace_range` for an existing property, four `insert`/`insert_str` for a new one) and the single `\nNAME:` needle is built on the stack. | 9-op edit, 1187-byte body | **70 → 2 allocs/op (68 fewer, ~35×) · 2.46× wall** (4255 → 1727 ns/op) |
| **F1** | `ResourceList` derived `itemIndexById = new Map(items.map((i,idx)=>[i.id,idx]))` — a fresh Map over the WHOLE accumulated list every infinite-scroll page (O(N)/page, Σ O(N²)), and being a new instance each page it also re-fired the reap-stale `$effect` (another O(N) id `Set`/page for a reap an append can never cause). New `ItemIndexBuilder` extends a persistent Map with the fresh page only and reuses the reference across appends. | 40 pages × 50 | **74.1 → 6.4 ms · 11.5× faster** index build across the drain |

## [C1] calendar-event edit — in-place iCal property rewrite

`calendar_storage_adapter::update_event` hydrates the stored event, then applies
each present field of the `UpdateEventDto` independently:

```rust
if let Some(summary) = update.summary { event.update_summary(summary)?; }
if let Some(description) = update.description { event.update_description(Some(description)); }
if let Some(location) = update.location { event.update_location(Some(location)); }
// …start/end (update_time_range), all_day (rewrites DTSTART+DTEND again), rrule…
```

Every one of those funnels into `CalendarEvent::update_ical_property` (an absent
field cleared → `remove_ical_property`), and the shipped-before body of that
method rebuilt the **entire** `ical_data` String on each call:

```rust
let search_str     = format!("\n{}:", property_name);     // needle 1
let search_str_alt = format!("\r\n{}:", property_name);   // needle 2 (redundant)
let pos = self.ical_data.find(&search_str).or_else(|| self.ical_data.find(&search_str_alt));
// …
let before = &self.ical_data[..value_start];
let after  = &self.ical_data[value_end..];
self.ical_data = format!("{}{}{}", before, value, after);  // a whole fresh body String
```

So a REST edit that changes summary + description + location + start + end +
all-day + rrule allocated **one full-body String per property** — and calendar
bodies run to ~11 KB once attendees / VALARMs are present — plus two throwaway
needles per call.

Two observations drive the fix:

1. **The `\r\nNAME:` needle is redundant.** `\nNAME:` is a *suffix* of
   `\r\nNAME:`, so `find("\nNAME:")` already matches a CRLF-terminated property
   line (returning the `\n` offset) — the `.or_else(find("\r\nNAME:"))` branch
   can never be reached. One needle suffices, and since iCal property names are
   short ASCII it is built into a 64-byte **stack** buffer (`line_needle`) — zero
   heap needle.
2. **The rewrite can be in place.** `replace_range(value_start..value_end, value)`
   is byte-for-byte what `before + value + after` produced, but it mutates the
   body's own buffer (growing once only when the new value is longer) instead of
   allocating a fresh body. The absent-property branch inserts the four pieces at
   one point in reverse (`\n`, value, `:`, name) after a single `reserve`, so a
   new property costs no fresh-body and no value-sized fragment either.

Because both arms edit the **same byte spans**, the emitted body is identical —
including the pre-existing quirk that editing a line on a CRLF body drops that
line's `\r` (the old span already included it; `replace_range` over the same
span preserves the behaviour exactly). The bench's equivalence gate asserts the
full 9-op edit is byte-identical, and the existing `calendar_event` unit tests
(`update_summary` / `update_time_range` / `update_all_day` round-trips) pin the
observable semantics.

Measured (`bench_round18_micro`, counting allocator, no Postgres). Both arms pay
one identical `base.to_string()` reset per op (a shared constant), so the
**fewer-allocs** figure is the pure per-edit saving.

```
## [C1] calendar-event multi-field edit (9 ops, 1187-byte body)
| arm                                            |        ns/op | allocs/op |
| BEFORE update_event in-place property rewrite  |       4255.4 |     70.00 |
| AFTER  update_event in-place property rewrite  |       1726.5 |      2.00 |
# 2.46x wall, 68.00 fewer allocs/op
```

The AFTER arm's two allocations per whole 9-op edit are the shared
`base.to_string()` reset and a single buffer grow (the longer DESCRIPTION value +
the inserted RRULE), versus 70 for the old per-property `format!` fan-out — a
35× cut, byte-identical output.

## [F1] ResourceList `itemIndexById` — incremental id→index Map

`ResourceList` pages its list in via infinite scroll (`items = [...items,
...page]`) and derived, on every change:

```js
const itemIndexById = $derived(new Map(items.map((i, idx) => [i.id, idx])));
```

`selectedItems` reads that Map to project the current selection in list order.
The derive is a full O(N) rebuild of a **fresh** Map over the whole accumulated
list every page — Σ O(N²) across a P-page drain with a selection active — and,
being a new instance each page, it also re-fired the reap-stale `$effect` (which
reference-diffs it), and that effect built *another* throwaway O(N) `Set` of ids
per page for a reap that an append can never trigger (an append only adds ids).

`ItemIndexBuilder` (new, `$lib/utils/itemIndex.ts`, mirroring
`ResourceSectionsBuilder`) uses the shared O(1) `isAppendExtension` witness: on
an append it indexes only the fresh tail into a persistent Map and returns the
**same reference**; any other change (reload, deletion, non-append) rebuilds into
a **new** Map. That reference contract is exactly what the two consumers want:

- `selectedItems` re-derives on every `items` change regardless (it indexes
  `items[idx]`), so it always reads the freshly-extended Map — a stable
  reference on append costs it nothing;
- the reap-stale `$effect` now tests membership against that Map instead of a
  fresh `Set`, and a stable reference on append means it **doesn't re-run** there
  (nothing to reap) while a rebuild — the delete/reload case — yields a new
  reference and **does** re-run it, precisely when stale ids must be dropped.

Gates (`round18.bench.test.ts`): the builder is asserted deep-equal to the
verbatim `buildItemIndex` reference at **every** page of a 12-page drain (and on
the final index of a 20-page one), a later duplicate id resolves to its highest
index (matching `Map`'s last-wins), and the reference-contract gate pins
same-ref-on-append / new-ref-on-rebuild. The perf gate requires the incremental
drain to beat the rebuild-per-page by ≥5×.

Measured: a 40-page × 50-item drain builds the index in **6.4 ms vs 74.1 ms —
11.5× faster** — and no longer churns a fresh Map + id-Set per page.

## Not shipped — deferred to a later round

Surfaced during the Round-18 audit but not landed (each needs its own decision,
fixture, or a different reactivity treatment):

- **Frontend — flat dotfile filter (`ResourceList` inline `items.filter` +
  `utils/dotfileFilter::filterDotfiles`, on photos):** the ROUND17 note flagged
  it as an O(N²) per-page rescan when *hide dotfiles* is on. Unlike the favorites
  `Set` (ROUND14 §F2) or this round's id-`Map`, the filtered result is a **flat
  array** that feeds `.filter`/rendering — reactivity needs a *fresh* array
  reference each page, and building one by `prev.concat(freshFiltered)` is itself
  O(N) (a mutate-in-place same-reference array would stop `photoRows` /
  `visibleItems` consumers from recomputing). There is no clean O(N)→O(page) win
  without partitioning the list; deferred pending a design (e.g. filtering per
  page in the loader and accumulating in page state, which changes the
  toggle-refilter semantics).
- **Frontend — `VirtualRows.offsets` prefix-sum (photos timeline):** rebuilt in
  full per page. An incremental prefix-sum needs (a) a version-counter to force
  `band`/`totalHeight` to recompute without a fresh `offsets` array reference
  (Svelte deriveds short-circuit on `===`), and (b) `PhotoTimeline` to guarantee
  row-object identity at the append boundary (a page that grows the last group
  re-lays-out its trailing strip row, breaking `isAppendExtension`). Both are
  real but want their own round; the raw numeric prefix-sum is also cheap (rows ≪
  photos), so this is lower-priority than the item-list rescans.
- **Backend query-shape (needs Postgres, carried from ROUND17):**
  `music_storage_adapter::list_public_playlists` 1 + N `COUNT(*)` fold; contact
  REST listings over-fetch the multi-KB `vcard` TEXT the `ContactDto` mappers
  never read (wants a *lite* row mapper).

## Environment / methodology

- `cargo run --release --features bench --example bench_round18_micro` —
  counting global allocator, no Postgres. Tunable: `BENCH_ITERS` (200000).
- `cd frontend && npx vitest run src/lib/components/round18.bench.test.ts` —
  equivalence, reference-contract, and wall-time perf gates.
- Each section is BEFORE (verbatim replica of the shipped-before shape) vs AFTER
  (verbatim replica of the shipped-after shape) with a byte/-value equivalence
  gate; the shipped source now matches each AFTER arm.
- Roll-back rule encoded per section: the Rust harness `std::process::exit(1)`s
  with `GATE FAIL … rollback` if an AFTER arm fails to reduce allocations; the
  vitest perf gate fails the test if the incremental arm isn't ≥5× faster.
