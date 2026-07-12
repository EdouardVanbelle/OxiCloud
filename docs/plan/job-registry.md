# Plan — Central `JobRegistry` + scheduler engine

## Context

OxiCloud runs several fire-and-forget background daemons today, each
spawned by a service factory in `src/common/di.rs` at startup:

| Service | Cadence | Purpose |
|---|---|---|
| `TrashCleanupService` | every 24 h | Delete trashed rows past retention; sweep orphan blobs |
| `StorageUsageService::start_reconciliation_job` | every 600 s | Rebuild `users.storage_used_bytes` and `drives.used_bytes` from `SUM(size)` |
| `db_pool_monitor` | every N s | Log pool saturation stats |
| `dedup_service` GC | on demand + inline | Reap orphan blobs past the grace window |
| `GrantCleanupService` (new — see `docs/plan/grant-cleanup.md`) | every 24 h | Delete `role_grants` rows past `expires_at + grace` |
| `tree_etag_flush_job` | every ~500 ms | Batch-bump folder ETags from a dirty queue |
| `content_index` worker | continuous | Drain indexing queue |
| `ConsistencyCheck` runs (see `docs/plan/consistency-check.md`) | admin-triggered in v1; scheduled once JobRegistry lands | Detect orphan/missing state across every stateful adapter — bidirectional, resumable, cursor-based |

Each daemon is a `tokio::spawn` + `tokio::time::interval` loop with:

- its own env var pattern (each service invents `_ENABLED`, `_INTERVAL_*`, `_GRACE_*`)
- its own logging schema (some `target: "audit"`, most plain `info!()`)
- its own admin trigger endpoint under `/api/admin/internal/*` (three
  today: `trigger-sweep`, `trigger-gc`, `trigger-grant-cleanup`), with
  duplicated boilerplate: gate check, admin guard, JSON envelope, 503
  when the service is disabled
- no observability surface for "what jobs are registered? when did they
  last run? what's their next-run ETA?"
- no way for plugins to declare their own scheduled work

The grant-cleanup PR (2026-07-12 session) surfaced this drift and we
agreed to defer the refactor rather than fold it into the security fix.
This plan captures the design so a future session can pick it up
without re-litigating decisions.

**Ordering vs `docs/plan/consistency-check.md`.** Consistency checks
ship FIRST — same session that surfaced this drift agreed on that
order. JobRegistry is a scheduling refactor with no user-visible
unblock; consistency closes an operator-visibility gap (orphan blob
leaks at scale, per issue #560's context) today. When JobRegistry
lands, it auto-consumes `ConsistencyRegistry::all_checks()` —
consistency v1 is admin-triggered only, no `tokio::spawn` interval.
The migration point below adds `consistency_checks` to the tenant
list once the framework is in place.

### Why the plugin angle matters

OxiCloud already supports Extism-based plugins with a lifecycle-hook
surface. The strongest single argument for a central scheduler is
turning it into a **stable extension point** — plugins declare
scheduled jobs in their manifest and the plugin lifecycle adapter
registers them at load time. Plugin jobs then show up alongside native
jobs in the admin listing, respect the same trigger endpoint, and log
under the same schema. Without a registry, plugins would each have to
manage their own timer loop inside the wasm sandbox (expensive) or use
host-provided cron primitives with no operational visibility.

## Design decisions (locked in by 2026-07-12 conversation)

### Scope of the first refactor

- **Grant-cleanup ships in the existing per-service style** (already
  merged). It is the reference tenant for the migration but not the
  first thing to migrate — that role goes to `TrashCleanupService`
  because it's the simplest self-contained loop and its admin surface
  (`trigger-sweep` maps 1:1 to trash purge only in shape) is a
  template for others.
- **Migration is phased**: land the registry + admin surface + one
  native tenant (trash) in one PR; migrate the remaining four
  (`storage-usage`, `db_pool_monitor`, `dedup GC`, `grant-cleanup`) in
  follow-ups. Plugins wire in only after the native migration is
  proven.

### Runtime model — one supervisor task, per-job panic containment

- **One `tokio::spawn`** at startup runs the scheduler main loop.
  Sleeps until the earliest due job, dispatches, sleeps again.
- Per-job **panic catching** via `tokio::task::spawn` inside the
  dispatch (or `AssertUnwindSafe` + `catch_unwind`). A bad job crashes
  its own run, not the scheduler.
- **Sequential dispatch within a tick** by default. Two jobs due at
  the same instant run one after the other. Parallel dispatch is a
  future toggle — most daemons touch the DB and don't benefit from it.
- Tokio task cost is not a factor (tasks share the worker pool; idle
  tasks cost ~200-500 bytes). The reason for a single supervisor is
  **operational**: one place to observe, one panic containment
  boundary, one config surface, one plugin-registration hook.

### Job identity

Every registered job has:

- `name: String` — stable identifier. Native jobs use snake_case
  (`trash_cleanup`, `storage_reconcile`, `grant_cleanup`). Plugin jobs
  are namespaced: `plugin:{plugin_id}:{manifest_job_name}` so plugin
  jobs never collide with native ones and can be bulk-unregistered on
  plugin unload.
- `owner: JobOwner` — either `Native` or `Plugin { plugin_id }`. Used
  by `unregister_by_owner()` for cascading cleanup and to surface who
  registered what in the admin listing.
- `interval: Duration` — how often the job fires. Loaded from env or
  manifest at registration time.
- `handler: Arc<dyn Fn() -> BoxFuture<'static, JobOutcome> + Send + Sync>`
  — the closure the scheduler invokes. Captures the service's `Arc`s
  (repositories, config, etc.).
- `timeout: Option<Duration>` — optional per-job wall-clock cap.
  Required for plugin jobs (see below); optional for native ones.

### `JobOutcome`

```rust
pub enum JobOutcome {
    Ok { count: u64, extra: serde_json::Value },
    Err(String),
    Timeout,
    Panicked(String),
}
```

Every outcome carries a `count` (rows processed, records touched,
whatever the job reports) plus a free-form `extra` JSON blob for
job-specific fields (e.g. `bytes_freed` on GC, `grace_days` on
grant-cleanup). The scheduler serialises this into a uniform log line
and stores the last outcome per job for admin introspection.

### Admin surface

```
GET  /api/admin/internal/jobs
  → [{ name, owner, interval_ms, last_run_at, last_outcome, next_run_at }]

POST /api/admin/internal/trigger-job/{name}?force=<bool>
  → { ok, outcome }  # runs one dispatch off-schedule
```

Both gated by the existing `OXICLOUD_ENABLE_ADMIN_INTERNAL_ENDPOINTS`
env var — reuses the same admin-guard middleware and the same
"disabled → 404" contract as today's per-service triggers.

The **existing per-service trigger endpoints** (`trigger-sweep`,
`trigger-gc`, `trigger-grant-cleanup`) stay as thin shims that call
`trigger-job/{name}` internally — backwards compatibility for the
existing Hurl suite and any operator scripts.

### Config surface

Each job carries its own subset of tunables (interval, retention/grace,
enabled). Convention:

```
OXICLOUD_JOB_<JOB_NAME>_ENABLED
OXICLOUD_JOB_<JOB_NAME>_INTERVAL_HOURS
OXICLOUD_JOB_<JOB_NAME>_<CUSTOM>...
```

Existing env vars keep working as **aliases** during migration —
`OXICLOUD_GRANT_CLEANUP_INTERVAL_HOURS` reads first, falls back to
`OXICLOUD_JOB_GRANT_CLEANUP_INTERVAL_HOURS`. Deprecated aliases stay
recognised through one minor version and warn on startup.

### Logging schema

Uniform structured target:

```rust
tracing::info!(
    target: "oxicloud::scheduler",
    event = "job.run",
    job = %name,
    owner = %owner,               // "native" | "plugin:<id>"
    outcome = %outcome_kind,      // "ok" | "err" | "timeout" | "panicked"
    count = ...,
    elapsed_ms = ...,
    // extras from the JobOutcome::Ok.extra map, flattened
    ...,
    "job {name} ran"
);
```

Security-relevant jobs (grant cleanup, authz cache invalidation) still
double-log to `target: "audit"` — the scheduler channel is for
observability; the audit channel is for compliance.

### Plugin integration

Plugin manifests gain a `[[jobs]]` section:

```toml
[[jobs]]
name = "external_drive_sync"    # Namespaced to `plugin:<id>:external_drive_sync`
interval_hours = 6              # OR interval_secs — one or the other
handler = "sync_drive"          # Exported wasm function name
timeout_secs = 300              # REQUIRED for plugin jobs
```

The **plugin lifecycle adapter** (already exists at
`src/application/adapters/plugin_lifecycle_hook.rs`) grows two calls:

- `on_plugin_loaded`: parse manifest jobs, call
  `job_registry.register(...)` for each with a handler that invokes
  `PluginRuntime::call_export(plugin_id, handler_name)`. Timeouts are
  enforced via `tokio::time::timeout`.
- `on_plugin_unloaded`: call
  `job_registry.unregister_by_owner(JobOwner::Plugin { plugin_id })`.

Extism call budgets (CPU, memory) already apply per-invocation; the
timeout is the wall-clock ceiling and MUST be shorter than the job's
interval so a stuck plugin doesn't miss ticks.

### Ordering and dependencies (deferred)

Cross-job dependencies (e.g. "trash cleanup runs before dedup GC")
are **not** modelled in v1. Every job runs independently. If a real
ordering constraint appears, we add a `depends_on: Vec<String>` field
and topological scheduling then.

### Shutdown coordination (deferred)

Matches the existing daemons: no cancellation channel. The scheduler
task dies with the runtime. If graceful shutdown lands elsewhere in
the codebase, the scheduler and all jobs migrate together.

## Approach

### 1. New module — `src/infrastructure/scheduler/`

```
src/infrastructure/scheduler/
    mod.rs               # Public API: JobRegistry, ScheduledJob, JobOwner, JobOutcome
    registry.rs          # Registry: register, unregister, unregister_by_owner, list, get
    engine.rs            # The supervisor loop: sleep-until-next-tick, dispatch, panic catch
    admin.rs             # Handlers: GET /jobs, POST /trigger-job/{name}
```

Split into files for testability — the registry is a pure data
structure, the engine is the async loop, the admin is the HTTP glue.

### 2. `JobRegistry` — the shared state

```rust
pub struct JobRegistry {
    jobs: RwLock<HashMap<String, RegisteredJob>>,
}

struct RegisteredJob {
    definition: ScheduledJob,
    last_outcome: Option<(chrono::DateTime<Utc>, JobOutcome)>,
    next_run_at: chrono::DateTime<Utc>,
}
```

`Arc<JobRegistry>` lives on `AppState` — one instance per process.
Native services register themselves during DI. Plugin adapter
registers on `on_plugin_loaded`.

### 3. Engine loop

```rust
async fn run(registry: Arc<JobRegistry>) {
    loop {
        let next = registry.pick_next().await;  // earliest next_run_at
        let sleep = next.deadline().saturating_duration_since(Instant::now());
        tokio::time::sleep(sleep).await;

        let outcome = registry.dispatch(&next.name).await;
        registry.record_outcome(&next.name, outcome).await;
    }
}
```

`dispatch` grabs the handler under a read lock, invokes it inside a
`spawn` + `catch_unwind`, applies the timeout, and returns the
`JobOutcome`. Sequential dispatch is intentional; two jobs due at the
same instant run one-after-the-other.

### 4. Native tenant migration — trash cleanup first

`TrashCleanupService::start_cleanup_job()` becomes
`TrashCleanupService::register(&registry)`. The trait method:

```rust
impl TrashCleanupService {
    pub fn register(self: Arc<Self>, registry: &JobRegistry) {
        registry.register(ScheduledJob {
            name: "trash_cleanup".into(),
            owner: JobOwner::Native,
            interval: Duration::from_secs(self.interval_hours * 3600),
            timeout: None,
            handler: Arc::new(move || {
                let this = self.clone();
                Box::pin(async move { this.run_once().await })
            }),
        });
    }
}
```

The service still owns its state; the registry just knows how to
invoke it. `di.rs`'s `create_trash_service()` calls `.register(&registry)`
instead of `.start_cleanup_job().await`.

Subsequent PRs port `storage-usage`, `db_pool_monitor`, `dedup GC`,
`grant-cleanup` the same way.

**Consistency-check runs — separate integration.** The consistency
framework (see `docs/plan/consistency-check.md`) exposes
`ConsistencyRegistry::all_checks() -> Vec<Arc<dyn ConsistencyCheck>>`.
For each check, JobRegistry registers a wrapping job:

```rust
for check in consistency_registry.all_checks() {
    registry.register(ScheduledJob {
        name: format!("consistency_{}", check.name()),
        owner: JobOwner::Native,
        interval: consistency_interval_for(check.name()),
        timeout: None, // consistency checks self-cancel via CheckStore
        handler: Arc::new(move || {
            let check = check.clone();
            let store = pg_check_store.clone();
            Box::pin(async move {
                // Resume the most-recent Paused run for this check, or
                // start a new one. Do NOT start a new run if one is
                // already Running — the scheduler is single-instance
                // per check.
                run_or_resume(check, store).await
            })
        }),
    });
}
```

Two subtleties for this integration:
- Consistency runs are long-lived (hours) — the scheduler MUST NOT
  block subsequent tick dispatch on the check completing. Scheduled
  invocation kicks the check off, records the run_id in `last_outcome`,
  and returns immediately; the check runs in its own spawned task.
- A check with a `Running` row when its tick fires must be SKIPPED, not
  overlapped. Overlapping two runs of the same check corrupts findings
  (both would write to different `run_id`s but scan the same resources
  in parallel; not incorrect but wasteful). The `run_or_resume` helper
  short-circuits with `JobOutcome::Ok { count: 0, extra: {"skipped":
  "already_running"} }`.

### 5. Admin trigger — one endpoint, three shims

New:

```rust
POST /api/admin/internal/trigger-job/{name}?force=<bool>
```

Existing (kept for back-compat):

```rust
POST /api/admin/internal/trigger-sweep       → trigger-job/storage_reconcile
POST /api/admin/internal/trigger-gc          → trigger-job/dedup_gc?force=...
POST /api/admin/internal/trigger-grant-cleanup → trigger-job/grant_cleanup?force=...
```

The shims stay pass-throughs so the existing Hurl suites continue to
work without changes. Deprecation warning surfaces via a
`Deprecation: true` response header operators can grep for.

### 6. Config parsing

New helper in `src/common/config.rs`:

```rust
fn env_job_var(job_name: &str, key: &str, alias: Option<&str>) -> Option<String> {
    env::var(format!("OXICLOUD_JOB_{}_{}", job_name.to_uppercase(), key))
        .ok()
        .or_else(|| alias.and_then(|a| env::var(a).ok()))
}
```

Each service uses it to load its own tunables plus its historical
env-var aliases. Aliases warn once at startup if used.

### 7. Plugin manifest schema

Extend the existing manifest parser (wherever plugin manifests are
parsed today — likely `src/infrastructure/services/plugins/`) to
recognise `[[jobs]]`. Validation:

- `name` matches `^[a-z][a-z0-9_]*$` (same shape as native names,
  namespaced with `plugin:<id>:` at registration).
- Exactly one of `interval_hours` / `interval_secs` is present.
- `handler` names an actual exported wasm function (validated by
  probing the plugin's exports at load time; missing export →
  `on_plugin_loaded` returns an error and the plugin is unloaded).
- `timeout_secs > 0` AND `timeout_secs < interval` (i.e. at least one
  full interval-worth of headroom).

## Critical files

**Create:**
- `src/infrastructure/scheduler/mod.rs` (~50 lines — pub types)
- `src/infrastructure/scheduler/registry.rs` (~120 lines)
- `src/infrastructure/scheduler/engine.rs` (~80 lines)
- `src/infrastructure/scheduler/admin.rs` (~100 lines — two handlers)

**Modify (v1 — migrate trash + wire admin surface):**
- `src/infrastructure/services/trash_cleanup_service.rs` — replace
  `start_cleanup_job` with `register(&registry)`. Returns `Self`
  (chainable) so DI stays terse.
- `src/common/di.rs` — build `Arc<JobRegistry>`, expose on `AppState`,
  spawn the engine, call `trash.register(&registry)`.
- `src/interfaces/api/handlers/admin_handler.rs` — add the two
  registry-backed handlers (list, trigger-by-name) alongside the
  existing per-service shims.
- `src/interfaces/api/routes.rs` — register `/internal/jobs` and
  `/internal/trigger-job/{name}`.
- `src/interfaces/api/mod.rs` — add utoipa paths.

**Modify (v2 — migrate remaining native tenants):**
- `src/application/services/storage_usage_service.rs`
- `src/infrastructure/services/db_pool_monitor.rs`
- `src/infrastructure/services/dedup_service.rs` (the GC daemon slice)
- `src/infrastructure/services/grant_cleanup_service.rs`

Each just replaces its `start_*_job` method with a `register`.

**Modify (v3 — plugin integration):**
- `src/application/adapters/plugin_lifecycle_hook.rs` — parse
  manifest jobs, register on load, unregister on unload.
- Plugin manifest schema doc (wherever it lives today).

**Modify (v4 — consistency-check auto-scheduling):**
- `src/common/di.rs` — after the consistency framework is wired,
  iterate `ConsistencyRegistry::all_checks()` and `registry.register(...)`
  each with the `run_or_resume` handler shown above.
- New helper `run_or_resume` on `PgCheckStore` (or `ConsistencyRegistry`):
  finds the latest `Paused` run for `check.name()`, resumes it, or
  starts a new one; short-circuits on `Running`.
- New env var `OXICLOUD_CONSISTENCY_{CHECK}_INTERVAL_HOURS` per check
  (default: `blobs` = 24 h, `thumbnails` = 24 h, `used_bytes` = 6 h,
  `folder_tree` = 24 h, `deep_hash` = 168 h weekly). Aliased under
  the JobRegistry `OXICLOUD_JOB_CONSISTENCY_{CHECK}_INTERVAL_HOURS`
  convention.

## Reused existing utilities

- **The `#[utoipa::path]` + admin-guard + gate pattern** already lives
  in `src/interfaces/api/handlers/admin_handler.rs`
  (`internal_trigger_gc`, `internal_trigger_grant_cleanup`) — the new
  registry-backed handlers copy the same shape.
- **The tokio task lifetime model** (spawn once, never joined, dies
  with the runtime) matches every existing daemon. Nothing new to
  learn.
- **Existing shim endpoints (`trigger-*`)** stay as thin wrappers, so
  the api-test suite doesn't need to change for the v1 landing.
- **`AGENTS.md` audit-logging convention** applies to
  `oxicloud::scheduler` observability lines — the security-audit
  channel is only for outcomes that belong on a compliance report;
  scheduler run lines are ops observability, not audit.

## Verification

1. **Compile**: `cargo check --all-features --all-targets` +
   `cargo clippy -- -D warnings` clean.
2. **v1 boot check**: start server; expect a single info line
   `scheduler started, N job(s) registered` where N is the count of
   migrated tenants (v1: just trash cleanup).
3. **v1 admin endpoint**:
   ```
   curl -s http://localhost:8086/api/admin/internal/jobs -H "Authorization: Bearer $TOKEN"
   ```
   returns a JSON array with the trash-cleanup entry, non-null
   `next_run_at`, `last_outcome` matching what actually happened at
   startup (first-tick-immediate).
4. **v1 Hurl regression**: run the existing api-test suite unchanged.
   `trigger-sweep` / `trigger-gc` / `trigger-grant-cleanup` all still
   work (they're thin shims). If any fail, the shim-forward layer has
   a bug.
5. **v1 shim deprecation**: the shims MUST emit a `Deprecation`
   response header. New Hurl asserts on that header.
6. **Panic containment**: unit test a job whose handler panics; the
   registry's `last_outcome` records `JobOutcome::Panicked`; the
   scheduler task is still alive (verified by triggering another job
   and seeing it run).
7. **Plugin integration (v3)**: a test plugin's manifest declares one
   job with a small `interval_secs`; load the plugin; observe the
   `plugin:<id>:job` in the admin listing; unload the plugin; observe
   the job disappear immediately and no further runs occur.
8. **Timeout enforcement (v3)**: a test plugin's job blocks longer
   than its declared timeout; `JobOutcome::Timeout` is recorded;
   subsequent runs proceed normally.

## Out of scope

- **Cron expressions**. Fixed intervals only. Real cron
  (day-of-week/month, arbitrary times) can layer on top later via a
  `next_run: Box<dyn NextRun>` trait — but nothing in OxiCloud needs
  it today.
- **Distributed scheduling**. Single-process only. If OxiCloud ever
  runs multi-node, `SELECT … FOR UPDATE SKIP LOCKED` on a lease table
  is the pattern; not now.
- **Backfill on startup**. If the process is down when a job's tick
  was due, we do NOT catch up — the job just runs at its next
  interval. Matches every existing daemon's behaviour today.
- **Job history**. Only the most recent outcome per job is stored,
  in memory. Longer history requires a table or a bounded ring buffer
  — deferred until a real diagnostic use case shows up.
- **Rate limiting the admin trigger endpoint**. It's already
  admin-gated; a malicious admin has bigger levers. Add if a
  legitimate concern surfaces.
- **Prometheus / OpenMetrics export**. Log-only for now. `job.run`
  events are the natural counter to expose whenever a metrics
  surface lands.
- **Graceful shutdown**. Matches existing daemons — the scheduler
  task dies with the runtime. If graceful shutdown lands globally,
  everything migrates together.

## Related memory notes

- `feedback_no_abbreviated_env_vars` — full-word env var names
  (`OXICLOUD_JOB_TRASH_CLEANUP_INTERVAL_HOURS`, not
  `OXICLOUD_JOB_TC_INTERVAL_H`).
- The grant-cleanup implementation is the closest reference for the
  daemon → tenant migration shape: three env vars, one impl of an
  authz trait method, one daemon service, one admin trigger. Same
  layers, same tests.
- `project_consistency_check_trait` — the consistency framework
  described in `docs/plan/consistency-check.md`, which ships FIRST
  and is auto-scheduled by JobRegistry via `all_checks()` once this
  plan lands. The two plans are designed to compose without
  retrofitting either.
