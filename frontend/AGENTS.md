# AGENTS.md — Frontend

Complements the repo-root `/AGENTS.md`. Not shipped (adapter-static
copies only `frontend/static/`).

## localStorage keys

Prefix `oxi-`, kebab-case separators. Example: `oxi-view-mode`.
Enforced by `$lib/utils/localStoragePrefs::wipeAppKeys()` which sweeps
every `oxi-*` key on user-account switches — any other prefix leaks the
previous user's state into the new one.
