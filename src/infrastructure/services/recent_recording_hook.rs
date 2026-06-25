//! Recording side of [`ResourceAccessHook`] — turns a successful file access
//! into a row in `auth.user_recent_files` via [`RecentService`].
//!
//! Wiring lives in `common/di.rs`: this hook is registered once, every
//! `_with_perms` file method on `FileRetrievalService` / `FileManagementService`
//! fans through it, and any future read-path or write-path service can opt in
//! by holding an `Option<Arc<dyn ResourceAccessHook>>` and calling
//! `on_file_accessed` after authZ.
//!
//! Two non-obvious behaviours, with rationale:
//!
//! * **Per-(caller, file) 60-second throttle.** Range-stream downloads send
//!   one GET per chunk (NC desktop, video seek, resumable transfers); without
//!   throttling each chunk would trigger an upsert against the same row.
//!   Moka's `time_to_live` gives us bounded memory and lock-free reads. The
//!   underlying `INSERT … ON CONFLICT DO UPDATE accessed_at = now()` is
//!   idempotent, so the rare TOCTOU window between `contains_key` and `insert`
//!   is harmless — at worst we record twice for the same instant.
//!
//! * **Fire-and-forget via `tokio::spawn`.** The `ResourceAccessHook` method
//!   is synchronous by contract (every `with_perms` caller would otherwise
//!   have to `await` the side-effect). The spawn lets the user-facing
//!   response return immediately; a DB hiccup in Recent recording never
//!   bubbles up to the GET / PUT that triggered it. Failures log at warn.

use std::sync::Arc;
use std::time::Duration;

use moka::sync::Cache;
use uuid::Uuid;

use crate::application::ports::recent_ports::RecentItemsUseCase;
use crate::application::ports::resource_access_hook::ResourceAccessHook;
use crate::application::services::recent_service::RecentService;

/// How long a successful recording suppresses repeat upserts for the same
/// `(caller, file)`. Sized to span a typical streamed range-GET burst while
/// still updating `accessed_at` often enough that the Recent list reflects
/// "this is the file I was just looking at".
const THROTTLE_TTL_SECONDS: u64 = 60;

/// Bound on simultaneous in-flight throttle entries. Each entry is a tuple
/// `(Uuid, String) -> ()` ≈ 80 B; 16 384 entries ≈ 1.3 MB worst case. LRU
/// eviction keeps memory bounded even if a pathological client touches a
/// million files in a minute.
const THROTTLE_MAX_ENTRIES: u64 = 16_384;

/// `ResourceAccessHook` implementation that records file accesses into
/// `auth.user_recent_files`, throttled per (caller, file).
pub struct RecentRecordingHook {
    recent: Arc<RecentService>,
    throttle: Cache<(Uuid, String), ()>,
}

impl RecentRecordingHook {
    pub fn new(recent: Arc<RecentService>) -> Self {
        // `support_invalidation_closures` is the moka opt-in needed by
        // `invalidate_entries_if` (the per-user throttle reset on
        // `on_recents_cleared`). Without it, the predicate-based
        // invalidate call silently no-ops and a freshly-cleared Recent
        // list refuses to re-record the same file until the TTL
        // expires — exactly the bug surfaced by tests/api/recent.hurl
        // step 8.
        let throttle = Cache::builder()
            .max_capacity(THROTTLE_MAX_ENTRIES)
            .time_to_live(Duration::from_secs(THROTTLE_TTL_SECONDS))
            .support_invalidation_closures()
            .build();
        Self { recent, throttle }
    }
}

impl ResourceAccessHook for RecentRecordingHook {
    fn on_file_accessed(&self, caller_id: Uuid, file_id: &str) {
        let key = (caller_id, file_id.to_string());
        if self.throttle.contains_key(&key) {
            return;
        }
        // Insert before spawning: even if the spawned task races with another
        // call for the same key, the cache entry suppresses the duplicate
        // before it reaches the DB. The ON CONFLICT clause covers the
        // sub-microsecond TOCTOU window between contains_key and insert.
        self.throttle.insert(key.clone(), ());

        let recent = Arc::clone(&self.recent);
        let (caller_id, file_id) = key;
        tokio::spawn(async move {
            if let Err(e) = recent.record_item_access(caller_id, &file_id, "file").await {
                tracing::warn!(
                    target: "oxicloud::recent",
                    caller_id = %caller_id,
                    file_id = %file_id,
                    "recent recording failed: {e}",
                );
            }
        });
    }

    fn on_recents_cleared(&self, caller_id: Uuid) {
        // Drop every throttle entry that would otherwise suppress the
        // next recording for this user. moka schedules the predicate to
        // run during the next maintenance pass — it's not synchronous.
        // The DB clear has already happened by the time we get here, so
        // any racing access between the clear and the next maintenance
        // pass just re-records via ON CONFLICT — the worst case is a row
        // that surfaces in Recent a few ms after the clear, which is
        // exactly what the user asked for.
        let _ = self
            .throttle
            .invalidate_entries_if(move |(k_caller, _), _| *k_caller == caller_id);
    }
}
