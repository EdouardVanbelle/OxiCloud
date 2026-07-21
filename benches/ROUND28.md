# Round 28 — extend the PROPFIND oc:id buffer (ROUND27 §H1) to the REPORT emit loops

A small follow-through: ROUND27 §H1 replaced the per-row `oc:id` `String` with one
reused `oc_buf` in the two NextCloud **PROPFIND** page loops, but the four
**REPORT** emit loops (`report_handler`) shared the identical per-row-String
shape and were explicitly deferred there. This round applies the same validated
transformation to them.

## The change

`report_handler`'s two REPORT handlers (`filter-files` favorites REPORT and
`search` REPORT) each emit a file loop and a folder loop, and each row did:

```rust
let oc_id = fid.map(|id| format_oc_id(id, file_id_svc));   // one String per row
…
write_{file,folder}_response(&mut xml, …, (fid, oc_id.as_deref()), …)
```

AFTER hoists one `oc_buf` per handler (reused across both its loops, beside the
same pattern the PROPFIND loops already use) and computes the id into it with
`format_oc_id_into` (added in ROUND27):

```rust
let mut oc_buf = String::new();          // once per handler
…
let oc_id: Option<&str> = match fid {
    Some(id) => { format_oc_id_into(&mut oc_buf, id, file_id_svc); Some(oc_buf.as_str()) }
    None => None,
};
write_{file,folder}_response(&mut xml, …, (fid, oc_id), …)
```

**1 String/row → 0** (amortized to one buffer per handler) across all four REPORT
loops. The `write_*_response` functions already take `Option<&str>`, so their
signatures are unchanged and the emitted `oc:id` bytes are byte-identical.

## Benchmark

This is the **same** transformation validated in ROUND27 §H1
(`bench_round27_micro`): a per-row `format_oc_id` String vs one reused buffer via
`format_oc_id_into`, byte-identical output. §H1 measured it on a 500-row page:

| arm    |    ns/op | allocs/op |
|--------|---------:|----------:|
| BEFORE | 34 185.3 |  1 000.00 |
| AFTER  | 14 484.9 |      2.00 |

**998 → 0 per-row allocs, 2.16–2.36× wall.** ROUND28 applies that proven change
to four more instances of the identical pattern (the REPORT loops), so no new
benchmark is needed — the §H1 gate is the evidence. REPORT/search is lower-traffic
than PROPFIND, so the aggregate impact is smaller, but it removes the last per-row
`oc:id` allocation from the NC emit surface.

## Not shipped — carried forward

- **`format_oc_id_into` for the trashbin per-item writer** (`write_trash_item_response`)
  would need the buffer threaded through its signature (it is a per-item fn, not a
  loop with a hoisted buffer); low traffic, deferred.
- **REPORT per-row `href` buffer** (`nc_href` allocates per row) — the ROUND20
  deferred href-buffer item; wants an `nc_href_into` + a precomputed encoded-user,
  a separate alloc pass.
- **S3 read zero-copy forward** — a genuine framing tradeoff (fewer, larger
  coalesced frames vs more, smaller zero-copy frames) that cannot be faithfully
  benchmarked without a real S3/MinIO fixture; not shipped on synthetic evidence.
- **Frontend folder-listing cache / `/resources` ETag** — the real bandwidth win
  needs a backend ETag on the listing feed + conditional 304, plus SWR wiring that
  respects cursor pagination. A dedicated backend+frontend feature.

## Environment / methodology

- Source-only extension of the ROUND27 §H1 change; the benchmark evidence is
  `bench_round27_micro` §H1. Verified: `cargo fmt --all --check` clean,
  `cargo clippy --features bench -- -D warnings` clean, `cargo test --lib
  --features bench` green.
