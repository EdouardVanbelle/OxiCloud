# Storage Fine Tuning

This page is for sysadmins who want to tune **where** OxiCloud places
upload data on disk and **why** the placement matters for throughput
and memory. The defaults work; the gains from a tuned layout are
significant on busy instances or constrained containers.

## The upload lifecycle in 30 seconds

```
        ┌─── direct (single-PUT / multipart) ───┐
client ─┤                                       │    Streamed DIRECTLY into the
        │                                       ├──► content-addressable store:
        │                                       │    CDC chunking + BLAKE3 +
        │                                       │    dedup checks happen while
        │                                       │    the bytes arrive. No spool
        │                                       │    file, no re-read; chunks
        │                                       │    the store already has are
        │                                       │    never written at all.
        │                                       │
        └─── multi-chunk upload ────────────────┤    Chunk PARTS accumulate on
             (`/api/uploads` /                  │    disk under OXICLOUD_CHUNK_DIR
              `/dav/uploads/...`)               │    until /complete, which
                                                │    streams them (in order)
                                                ▼    through the same CDC path.
                                  ┌─────────────────────────┐
                                  │ STORAGE BACKEND          │
                                  │  • local FS (.blobs/)    │
                                  │  • S3-compatible         │
                                  │  • Azure Blob            │
                                  └─────────────────────────┘
```

Two practical consequences:

- **Direct uploads no longer use a spool directory.** Each uploaded
  byte is written at most once — straight into the blob backend as a
  CDC chunk. Re-uploads and edited files write only the chunks the
  store doesn't already have.
- **The chunk-session directory sees write-heavy churn** during
  multi-chunk uploads — fast disk (NVMe) and sufficient free space
  matter more here than on the final storage backend.

## Upload size caps — what each one bounds

Three independent caps control how large an upload OxiCloud will
accept. Pick them with disk sizing in mind: the chunk directory must
be able to hold the worst case (cap × concurrent sessions).

| Variable | Default | What it caps | When it fires |
|---|---|---|---|
| `OXICLOUD_MAX_UPLOAD_SIZE` | 10 GB | **Whole-file ceiling.** Applies to both direct PUT (per-body) and chunked uploads (declared `total_size`). The absolute upper bound on any single file in OxiCloud. | Chunked: at `POST /api/uploads` against the JSON-declared `total_size`, before any chunk is uploaded. Direct PUT: indirectly via `OXICLOUD_DIRECT_PUT_MAX_BYTES`, which is expected to be ≤ `OXICLOUD_MAX_UPLOAD_SIZE`. |
| `OXICLOUD_DIRECT_PUT_MAX_BYTES` | 1 GiB | **Non-chunked PUT body.** Per-request cap for `PUT /webdav/...` and `PUT /remote.php/dav/files/.../...`. Set below `OXICLOUD_MAX_UPLOAD_SIZE` so larger files are pushed onto the chunked protocol — which is resumable on failure. | During body streaming, as a per-frame accumulator. Excess → 413 with a "use chunked upload" hint. |
| `OXICLOUD_CHUNK_MAX_BYTES` | 100 MB | **Per-chunk body** in a chunked-upload session (`PATCH /api/uploads/{id}` or `PUT /remote.php/dav/uploads/.../chunk`). Independent of the whole-file cap — a 5 GB file in 100 MB chunks is 50 PATCHes each bounded by this. | During chunk-body streaming. Excess → 413. |

### Recommendation: prefer chunked uploads for large files

The defaults (`OXICLOUD_DIRECT_PUT_MAX_BYTES` = 1 GiB, well below
`OXICLOUD_MAX_UPLOAD_SIZE` = 10 GB) are deliberately asymmetric.
Files between those two caps can only succeed via the chunked
protocol. The reason is **resilience**: a direct PUT at 95 % of 5 GB
that drops loses everything (the partially ingested chunks are
reclaimed by GC, but the client must restart from byte 0). The same
drop on a chunked upload loses one ~5 MB chunk; the client retries
that chunk and continues. NextCloud desktop and the OxiCloud web UI
already switch to chunked at ~10 MB (`CHUNKED_UPLOAD_THRESHOLD`).

### Disk sizing

OxiCloud streams bodies frame-by-frame, so **RAM** is bounded
(~10 MB per in-flight upload for the CDC ingest buffers) regardless
of the caps. **Disk space** scales with the caps:

- **Direct PUT / multipart**: no transient spool. Bytes land directly
  in the blob backend as deduplicated chunks; worst-case extra disk
  per upload is the file's own (deduplicated) size — the same space
  the stored file occupies afterwards.
- **Chunked upload**: each in-flight session accumulates its chunk
  parts under `OXICLOUD_CHUNK_DIR` until `/complete` streams them
  into the blob store and the session is cleaned up. Worst case disk
  per session = **file_size** (the parts); total =
  `OXICLOUD_MAX_UPLOAD_SIZE × concurrent_chunked_sessions`.

| Settings | Chunked worst case (5 sessions) | Safe on 4 GB volume? |
|---|---|---|
| Defaults: `OXICLOUD_MAX_UPLOAD_SIZE`=10 GB | 50 GB | ❌ overflows |
| `OXICLOUD_MAX_UPLOAD_SIZE`=500 MB | 2.5 GB | ✅ fits |

### Don't put `OXICLOUD_CHUNK_DIR` on tmpfs

In many container setups the OS temp dir is **tmpfs** — RAM-backed
storage that counts against the cgroup memory limit. A few concurrent
multi-GB chunked sessions on tmpfs will wake the OOMKiller long
before the uploads finish. Point `OXICLOUD_CHUNK_DIR` at a real-disk
directory in containers.

## TL;DR

| Variable | Default | Purpose |
|---|---|---|
| `OXICLOUD_STORAGE_PATH` | `./storage` | Where `.blobs/` lives (the canonical content store) |
| `OXICLOUD_CHUNK_DIR` | `{STORAGE_PATH}/.uploads` | Where chunked-upload sessions accumulate |

The two rules that matter most:

1. **Keep `OXICLOUD_CHUNK_DIR` off tmpfs** (the default in many
   containers) — chunk parts count against the cgroup memory limit
   and can trigger OOMKill on multi-GB uploads.
2. **NVMe for the chunk dir pays off** on deployments with heavy
   large-file traffic: each chunk PUT writes a file and the progress
   bitmap, and `/complete` reads them all back in order.

## Where each upload surface writes

| Surface | Default destination | Configurable via |
|---|---|---|
| REST chunked PUT (`PATCH /api/uploads/{id}`) | `{STORAGE_PATH}/.uploads/{upload_id}/chunk_NNNNNN` | `OXICLOUD_CHUNK_DIR` |
| NextCloud chunked PUT (`PUT /dav/uploads/.../chunk`) | `{STORAGE_PATH}/.uploads/nextcloud/{user}/{upload_id}/{chunk_name}` | `OXICLOUD_CHUNK_DIR` |
| Direct PUT / multipart / WOPI / chunked `/complete` | straight into the blob backend (CDC chunks) | `OXICLOUD_STORAGE_PATH` (local backend) |
| Final blob storage | `{STORAGE_PATH}/.blobs/{ab}/{abc…}.blob` | `OXICLOUD_STORAGE_PATH` |

The local blob backend stages each chunk write under
`{STORAGE_PATH}/.dedup_temp/` and promotes it with an atomic
`rename(2)` — both directories live under `OXICLOUD_STORAGE_PATH`,
so same-filesystem placement (and therefore atomic promotion) is
automatic and not separately configurable.

## Recommended layouts

### Single-disk box (most common)

Defaults are fine:

```bash
OXICLOUD_STORAGE_PATH=/var/lib/oxicloud
# OXICLOUD_CHUNK_DIR unset → /var/lib/oxicloud/.uploads
```

### Container with constrained memory

Critical: make sure the chunk dir doesn't sit on tmpfs.

```bash
OXICLOUD_STORAGE_PATH=/data
OXICLOUD_CHUNK_DIR=/data/.uploads
```

### Split-disk (NVMe intake + HDD blobs)

```bash
OXICLOUD_STORAGE_PATH=/mnt/hdd/oxicloud      # .blobs/ + .dedup_temp/
OXICLOUD_CHUNK_DIR=/mnt/nvme/oxi-chunks
```

Chunk parts land on NVMe (fast PUTs, fast `/complete` read-back);
the deduplicated chunks are written once to the HDD-backed blob
store as `/complete` streams through them.

## Sharing the chunk directory

The REST and NC chunked surfaces can share `OXICLOUD_CHUNK_DIR` by
design. Each writer tags its output so they never interfere:

| Writer | On-disk name pattern |
|---|---|
| REST chunked sessions | `oxi-chunk-{uuid}/` — directories with a well-known prefix |
| NC chunked subtree | `nextcloud/{user}/{uuid}/` — under its own root subdir |

The 24-hour orphan-session cleanup loop filters strictly on the
`oxi-chunk-` prefix, so it can NEVER delete a non-OxiCloud directory
that happens to live alongside chunked sessions.

## Quick verification

Boot the server with `RUST_LOG=info` and the first lines after the
banner include:

```
oxicloud: Upload limits loaded from config max_upload_size_mb=10240 chunk_max_bytes_mb=100
```

That confirms the upload-cap env vars were read. To confirm
directory placement, watch for chunk file creation under your
`OXICLOUD_CHUNK_DIR` (or its default `{STORAGE_PATH}/.uploads/`)
during a chunked upload — `ls` while a sync is in progress shows the
`{uuid}/chunk_NNNNNN` files appearing in real time.
