//! Observer notified when a caller successfully reads or mutates a file.
//!
//! Read-event sibling of [`crate::application::ports::file_lifecycle`]. The
//! lifecycle hook fires on content changes (created/copied/updated/deleted);
//! this one fires on access — every authorised file read, every successful
//! upload, every PUT/COPY — and lets cross-cutting observers (Recent list,
//! audit trail, future "last seen by" UX) react without each
//! protocol-surface handler having to remember to call them.
//!
//! Folders are deliberately out of scope: a listing fires on every UI
//! navigation, every PROPFIND, every NC sync poll, and would dominate
//! `auth.user_recent_files` with noise that no user actually opened.
//! Only file-level interactions count.
//!
//! Implementors run **after** the service layer's authZ check has passed and
//! the read/write has succeeded; a denied or 404'd request never fires the
//! hook. The method is synchronous — implementors that need to do real work
//! spawn it themselves so the user-facing request is never blocked on the
//! side-effect. The recording impl lives in
//! `infrastructure/services/recent_recording_hook.rs`.

use uuid::Uuid;

/// Fired by the application services on a successful, authorised access to a
/// file owned (or shared with) the caller.
///
/// `caller_id` is mandatory because the recording side needs to know **who**
/// touched the file — the same file accessed by two different users records
/// two separate Recent rows. Anonymous surfaces (public share downloads via
/// `/api/s/{token}`) deliberately do not call this hook: a "viewer" without
/// an authenticated identity has no Recent list to land in.
pub trait ResourceAccessHook: Send + Sync {
    /// Called after a file read or successful write touched `file_id` on
    /// behalf of `caller_id`. The caller has already been authorised — the
    /// hook is fire-and-forget; failures are the implementor's problem and
    /// must never propagate.
    fn on_file_accessed(&self, caller_id: Uuid, file_id: &str);

    /// Called after `caller_id` has emptied their Recent list (either by
    /// clearing the whole table or removing a single row). Implementors
    /// hold in-memory throttle / dedup state keyed by `(caller, item)`;
    /// without this signal a freshly-cleared list would refuse to record
    /// the next access until the throttle TTL expires, leaving the user
    /// staring at an empty Recent and wondering why their open-then-close
    /// did nothing.
    ///
    /// Default no-op: implementations without any in-memory state — most
    /// audit-trail-style observers — needn't react.
    fn on_recents_cleared(&self, _caller_id: Uuid) {}
}
