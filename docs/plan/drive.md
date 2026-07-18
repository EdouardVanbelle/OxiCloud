# Drives — design proposal

> **Status**: design proposal, locked but not implemented. Reviewed Jun 2026 with
> Ed; all blocking decisions answered. Open items listed at the end of the file.

## Context

Today every resource (folder, file) in OxiCloud has a single user owner
recorded directly on the row (`storage.folders.user_id`,
`storage.files.user_id`). Sharing is layered on top via ReBAC grants in
`storage.access_grants`. The model is simple and has carried us this far,
but it has two structural problems:

1. **Owner = user, full stop.** When a user leaves the company, their
   files leave with them. The team's de-facto shared folders are
   technically owned by a single person; transferring ownership requires
   moving every resource one-by-one and re-issuing every grant. There is
   no "this folder belongs to the Engineering team" concept.

2. **Quota is per-user, full stop.** A user with personal storage who
   also collaborates in a 1 TB team space currently has both counted
   against their personal quota (or the team data costs are absorbed by
   whoever owns the folder). There is no way to bill team storage to the
   team.

The proposal is to introduce **drives** as a Google-Workspace-style
container concept:

- Every resource belongs to exactly one drive.
- A drive has owners (users **and/or** groups) with roles.
- Each user automatically gets a personal drive at registration.
- Shared drives can be owned by groups, so membership changes
  automatically follow the group — "Bob left" no longer needs an admin
  to chase down which folders to reassign.
- Quotas move from users to drives.
- Per-drive policies enable team-level rules (forbid public links,
  forbid sharing, forbid cross-drive moves, future: timeboxed sessions,
  end-to-end encryption).

The shift is large but the model is well-understood — it's the same
pattern Google Workspace's Shared Drives and Microsoft SharePoint
Document Libraries use. OxiCloud users coming from those services will
recognise it instantly.

## Prerequisite — PR D-Prep: `access_grants → role_grants`

**Status (2026-06-17): scope complete, ready to PR.** The schema migration
runs cleanly against real sandbox data (38 role_grants rows produced,
matching the audit's 29 viewer / 5 editor / 4 owner distribution, zero
bundle mismatches). End-to-end Hurl test (`tests/api/role_grants.hurl`)
covers create / atomic role update / revoke with both the canonical
`"owner"` and the legacy `"admin"` compat wire format. Engine reads
pivot to `role_grants` for authz decisions; `access_grants` stays
populated via dual-write as the safety net for one release cycle.
**Ratified 2026-06-16.** A separate PR lands BEFORE D0 that refactors
`storage.access_grants` into `storage.role_grants` with role-bundle
semantics:

```sql
storage.role_grants
    id            uuid PRIMARY KEY DEFAULT gen_random_uuid()
    subject_type  text NOT NULL CHECK (subject_type IN ('user','group'))
    subject_id    uuid NOT NULL
    resource_type text NOT NULL  -- 'folder','file','drive','group',…
    resource_id   uuid NOT NULL
    role          text NOT NULL CHECK (role IN ('viewer','editor','owner', …))
    expires_at    timestamptz NULL
    granted_by    uuid NOT NULL FK → auth.users(id)
    granted_at    timestamptz NOT NULL DEFAULT now()
```

Roles map to permission bundles via a single
server-side function `role_bundle(role) -> &[Permission]`:

| Role | Bundle |
|---|---|
| `viewer` | `Read` |
| `editor` | `Read, Create, Update, Comment` |
| `owner` | `Read, Create, Update, Comment, Delete, Share, Manage` |

`Manage` is a new Permission introduced in this refactor — covers
"configure this resource's settings, add/remove members, change role
assignments". Future-friendly for Group-as-Resource.

**Why first**:
1. Refactor has standalone value beyond Drive (cleaner API, audit
   log, UI surface).
2. Doing it AFTER Drive would force migrating both `access_grants`
   AND `drive_members` — solving the unification first means D0
   migrates one table, not two.
3. **The `drive_members` table planned by earlier drafts goes away.**
   Drive membership becomes rows in `role_grants` with
   `resource_type='drive'`. One table, one engine, one cache.

**The sections below describe the Drive model assuming D-Prep has
landed.** References to "drive membership" mean rows in
`role_grants` with `resource_type='drive'`, not a separate table.

**Gating policy**: D-Prep must ship, bake in production, and pass
validation across the API surface (grant endpoints, audit log
events) and the UI surface (My Shares dialog, share modals, admin
group/role views) before any of the Drive PRs (D0–D7) begin. The
goal is to surface and absorb refactor-related issues against the
existing single-resource model — not to discover them mid-flight
while the Drive migration is also in motion. If D-Prep needs
follow-up patches after deploy, they land as standalone fixes
before D0 starts.

## Locked design (decisions ratified in design discussion)

### 1. Ownership pivot

- A new table `storage.drives` becomes the ownership anchor.
- `storage.folders` and `storage.files` lose their `user_id` column;
  they gain `drive_id NOT NULL`.
- The owner of a resource is computed via `resource.drive_id →
  drive.drive_members WHERE role='owner'` — a JOIN, not a denormalised
  column. The drives table will be tiny (one row per internal user +
  handful of shared drives), so the join is cheap with a proper index.
- **Single source of truth** confirmed. `user_id` is dropped from
  resources after a migration phase (see Migration below).

### 2. Drive membership

Drive membership lives in `storage.role_grants` (introduced by PR
D-Prep — see prerequisite above) with `resource_type='drive'` and
`resource_id=<drive uuid>`. There is **no** separate
`drive_members` table. One row per `(subject, drive)` pair carries
the role; the role expands to a permission bundle via the same
`role_bundle()` function used everywhere else.

Concretely, a drive membership is:

```sql
INSERT INTO storage.role_grants
       (subject_type, subject_id, resource_type, resource_id, role, granted_by)
VALUES ('user',       $user_id,   'drive',       $drive_id,   'editor', $admin_id);
```

The `PgAclEngine` reads this row no differently than any other
grant; the drive-membership concept exists only at the application
layer (`DriveService` enforces the personal-drive invariants, etc.)
— there's no separate engine path or cache keyspace.

- A **shared** drive (`kind='shared'`) can have **0..N user
  owners + 0..N group owners** (mixed freely) plus any number of
  editors/viewers.
- A **personal** drive (`kind='personal'`) has exactly **one**
  member row in `drive_members`, fixed to `(subject_type='user',
  subject_id=<the user>, role='owner')`. The rule applies to
  every `kind='personal'` row, whether it carries
  `default_for_user IS NOT NULL` (the user's default drive) or
  `default_for_user IS NULL` (a secondary personal silo, e.g. a
  SQL-created sibling root folder promoted by migration §10).
  Secondary does **not** loosen the single-user constraint.
  - Personal drives **cannot** be added to, role-changed, or have
    their sole owner removed. They cannot be co-owned. They cannot
    be transferred to another user. This matches Google Drive's
    "My Drive" and Microsoft's "OneDrive" semantics: a personal
    drive is a single-user namespace; collaboration happens through
    per-resource grants, by moving content into a shared drive,
    or by promoting a secondary personal drive to `kind='shared'`
    (capability matrix in §3).
  - Enforcement is at the application layer:
    `DriveService::add_member` refuses when `kind='personal'`;
    `remove_member` refuses when the drive is personal. User
    deletion cleans up the user's **default** personal drive via
    the `default_for_user` FK cascade, and the user's
    **secondary** personal drives via an application-layer pass
    (those have no FK to cascade through — see §6 lifecycle).
- **Shared-drive last-owner protection**: removing the final
  `role='owner'` member of a shared drive is refused at the
  application layer. The check counts remaining owner-role members
  (expanding group owners to their members) and rejects the delete
  if the count would become zero.
- A shared drive can have **0 viewers** and **0 editors** — only
  the ≥1-owner invariant matters.

### 3. Drive entity — pure metadata + a 1:1 root folder

A drive is a **metadata-only holder** (quota, kind, policies, default flag)
paired 1:1 with a *root folder* that owns the drive's visible identity
(name, path materialisation, ltree anchor). The drive itself has no
`name` column — every property the user thinks of as "the drive"
(its display name, its containing children, its location in the
ltree) lives on the root folder row.

This is the Unix-philosophy split: the *filesystem volume* is the drive
(quota, policies, ownership metadata); the *mount point* is the root
folder (name, hierarchy, paths). Clients interact with the root folder
through the standard folder API — no special "drive root" endpoint, no
polymorphic creation surface, no "create at drive vs in folder" duality.

```sql
storage.drives
    id                uuid PRIMARY KEY DEFAULT gen_random_uuid()
    kind              text NOT NULL CHECK (kind IN ('personal','shared'))
    default_for_user  uuid NULL FK → auth.users(id) ON DELETE CASCADE
    quota_bytes       bigint NULL                         -- NULL = unlimited
    used_bytes        bigint NOT NULL DEFAULT 0
    policies          jsonb NOT NULL DEFAULT '{}'
    -- The drive's mount-point folder. Nullable AT THE COLUMN TYPE LEVEL
    -- only because the column is set mid-statement during atomic
    -- creation (see "Atomic creation" below) — invariant: after any
    -- successful create_personal_drive() call, this is non-NULL. Code
    -- that reads drives can treat it as Uuid in Rust.
    root_folder_id    uuid NULL FK → storage.folders(id) ON DELETE CASCADE
    created_at        timestamptz NOT NULL DEFAULT now()
    updated_at        timestamptz NOT NULL DEFAULT now()
    -- `default_for_user` may ONLY be set on personal drives.
    -- Shared drives have it NULL by definition.
    CONSTRAINT drives_default_marker_personal_only
        CHECK (default_for_user IS NULL OR kind = 'personal')

CREATE UNIQUE INDEX drives_default_for_user_idx
    ON storage.drives (default_for_user)
    WHERE default_for_user IS NOT NULL;  -- one DEFAULT personal drive per user
```

#### Drive name lives on the root folder

`storage.drives` has no `name` column. The drive's display name is
`SELECT f.name FROM storage.drives d JOIN storage.folders f
ON f.id = d.root_folder_id WHERE d.id = $drive_id`.

Why this is the right shape:

- **Single source of truth.** No duplication between `drives.name` and
  `folders.name`, no drift risk, no decision about which side to
  serve when they differ.
- **Renaming is the standard folder API.** `PATCH /api/folders/<root_id>`
  with a new name renames the drive. No separate
  `PATCH /api/drives/<id>/name` endpoint. The lifecycle / cascade
  / search-index behaviour that already exists on folder rename
  applies automatically.
- **No "rename the drive but not its mount point" footgun.** They're
  always in sync because they're the same column.

Identity is still carried by `kind` + `default_for_user` — *not* by
the name. UI and NC default-drive resolution query on `kind` /
`default_for_user`, never on `name = 'Personal'`. Renaming "Personal"
→ "Ed's space" preserves identity; only the label changes.

The migration-time default for personal drive root folder names is
`'Personal'`. Secondary personal drives (sibling roots from the M2
backfill — see §10) carry over whatever name the original sibling
root folder had.

#### Two orthogonal properties: `kind` and `default_for_user`

- **`kind`** = drive capability shape (see §2 for the rules):
  - `'personal'` = single-user single-owner; cannot have members
    added; sharing happens only through per-resource grants.
  - `'shared'` = multi-owner / multi-member; supports user and
    group members at every role.
- **`default_for_user`** = "this is the default drive for user
  X". Set on exactly one `kind='personal'` drive per user (the
  partial unique index enforces it). Used by:
  - UI default-drive redirect (`/` → `/drive/<default>`).
  - NC default-drive resolution when the credential doesn't pin
    a specific drive.
  - The `ON DELETE CASCADE` on this column cleans up the user's
    default drive in one step when the user is deleted.
  - Canonical lookup: `SELECT id FROM storage.drives WHERE
    default_for_user = $1` — single-row, index-backed.

A user can have **multiple `kind='personal'` drives** — at most
one of them carries `default_for_user`, the others sit as
"secondary personal" silos (e.g. SQL-created root folders
promoted by migration §10). Secondary personals follow the same
single-owner rules as the default; they just aren't the default
landing target.

External users **never** get a personal drive (lifecycle §6).
There is no constraint to write — externals simply have no row
in `storage.drives` with `default_for_user = <their id>`, and
nothing tries to create one.

#### Atomic creation — single transaction, four writes

A drive and its root folder reference each other circularly:
`storage.drives.root_folder_id` points at `storage.folders.id`, and
`storage.folders.drive_id` points at `storage.drives.id`. Creating
them naively could leave inconsistent half-state on a server crash
mid-sequence: drive without folder, folder without drive, or either
without an owner role_grant.

The repo's `create_personal_drive_atomic` wraps the four writes in
a single transaction so they commit together or not at all:

1. INSERT drive (with `root_folder_id = NULL`) → returns drive id.
2. INSERT folder (with `drive_id` = the drive's id) → returns folder id.
3. UPDATE drive SET `root_folder_id` = the folder id.
4. INSERT role_grant (owner, subject = caller, resource = drive).
5. COMMIT.

Why a transaction rather than one CTE statement: PostgreSQL's CTE
sub-statements all read the target tables from the *same snapshot*
— a later sub-statement's `UPDATE storage.drives WHERE id = …`
cannot match a row inserted by an earlier sub-statement, even if
the earlier statement returned the new id via `RETURNING`. The
documented escape hatch (`DEFERRABLE INITIALLY DEFERRED` FKs +
pre-generated UUIDs) is the alternative but adds constraint
plumbing to support a single uncommon code path. A transaction is
boring and correct.

Crash safety: any failure between steps 1 and 4 rolls back — no
drive without folder, no folder without drive, no drive without
owner. Once step 5 commits, the invariant holds.

For reference, the equivalent (broken) one-CTE form looks like:

```sql
WITH new_drive AS (
    INSERT INTO storage.drives
        (kind, default_for_user, quota_bytes, policies)
    VALUES ('personal', $user_id, NULL, '{}'::jsonb)  -- personal drives carry
                                                       -- NULL quota; the cap is
                                                       -- the user envelope, §7
    RETURNING id
),
new_root AS (
    INSERT INTO storage.folders
        (name, parent_id, user_id, drive_id, created_by, updated_by)
    SELECT 'Personal',     -- root folder name
           NULL,            -- parent_id (this IS the root)
           $user_id,
           new_drive.id,    -- forward-ref to the drive's id
           $user_id, $user_id
      FROM new_drive
    RETURNING id, drive_id
),
drive_updated AS (
    UPDATE storage.drives d
       SET root_folder_id = new_root.id
      FROM new_root
     WHERE d.id = new_root.drive_id
    RETURNING d.id
),
new_grant AS (
    INSERT INTO storage.role_grants
        (subject_type, subject_id, resource_type, resource_id, role, granted_by)
    SELECT 'user', $user_id, 'drive', du.id, 'owner', $user_id
      FROM drive_updated du
    RETURNING resource_id
)
SELECT d.id, d.root_folder_id, d.kind, d.default_for_user,
       d.quota_bytes, d.used_bytes, d.policies,
       d.created_at, d.updated_at
  FROM storage.drives d
  JOIN new_grant g ON g.resource_id = d.id;
```

Why the one-CTE form above does NOT work — and what we ship instead:

The shared-snapshot rule (`postgresql.org/docs/current/queries-with.html`
§7.8.2: "they cannot 'see' one another's effects on the target tables")
breaks the `drive_updated` sub-statement. Its `UPDATE storage.drives d
… WHERE d.id = new_root.drive_id` evaluates `WHERE d.id = …` against
the snapshot, which doesn't contain the drive inserted by `new_drive`.
The UPDATE matches zero rows; `RETURNING` returns zero rows;
`new_grant` (which feeds off `drive_updated`) inserts zero role_grants;
the final SELECT joins on an empty CTE branch and returns nothing.
Symptoms in tests: drives exist with `root_folder_id IS NULL`, owners
have no `role_grants` row, `/api/drives` returns `[]`.

The fix is the four-step transaction described above. Rust:

```rust
let mut tx = pool.begin().await?;
// Personal drives carry NULL quota_bytes — the cap is the user envelope
// (`auth.users.storage_quota_bytes`, §7), not the per-drive column.
let drive_id: Uuid = sqlx::query_scalar(
    r#"INSERT INTO storage.drives (kind, default_for_user, quota_bytes)
       VALUES ('personal', $1, NULL) RETURNING id"#,
).bind(owner).fetch_one(&mut *tx).await?;

let folder_id: Uuid = sqlx::query_scalar(
    r#"INSERT INTO storage.folders
        (name, parent_id, user_id, drive_id, created_by, updated_by)
       VALUES ('Personal', NULL, $1, $2, $1, $1) RETURNING id"#,
).bind(owner).bind(drive_id).fetch_one(&mut *tx).await?;

sqlx::query("UPDATE storage.drives SET root_folder_id = $1 WHERE id = $2")
    .bind(folder_id).bind(drive_id).execute(&mut *tx).await?;

sqlx::query(
    r#"INSERT INTO storage.role_grants
        (subject_type, subject_id, resource_type, resource_id, role, granted_by)
       VALUES ('user', $1, 'drive', $2, 'owner', $1)"#,
).bind(owner).bind(drive_id).execute(&mut *tx).await?;

tx.commit().await?;
```

Each statement sees the prior statements' writes (transaction-local
visibility, not the CTE shared snapshot). FK timing works without
`DEFERRABLE`: each FK is satisfied at the moment its row is written
because the referenced rows already exist.

#### DB-level invariant: no orphan root folder

`storage.drives.root_folder_id` is NULLable at the column level
(required by the four-write transaction above — the drive INSERT
can't reference a folder that doesn't exist yet). The "every drive
has a root folder" invariant is application-enforced by the atomic
creation transaction being the only mint path. But the symmetric
invariant — "every root folder belongs to a drive" — is *not* free
either: a `storage.folders` row with `parent_id IS NULL` whose
`drive_id` doesn't point at a drive whose `root_folder_id` equals
its `id` would be an orphan that the resolver can't reach (the
chroot can't land on it) but that still occupies the
`(parent_id IS NULL, name, drive_id)` unique slot.

D0 lands a DB-level guard for this case, as a defence-in-depth
layer behind the application invariant. Implementation: an
`AFTER INSERT OR UPDATE` constraint trigger on
`storage.folders` that fires when `NEW.parent_id IS NULL` and
refuses unless:

```sql
EXISTS (
    SELECT 1 FROM storage.drives
     WHERE id = NEW.drive_id
       AND root_folder_id = NEW.id
)
```

Declared `DEFERRABLE INITIALLY DEFERRED` so the four-write
transaction's order (folder INSERTed at step 2, drive's
`root_folder_id` UPDATEd at step 3) doesn't trip the trigger
mid-transaction — the check fires at COMMIT, by which point both
sides of the cycle are wired.

Test coverage: a Hurl/integration test that hand-rolls
`INSERT INTO storage.folders (… parent_id=NULL, drive_id=<existing>)`
*without* the matching drives.root_folder_id update — asserts the
trigger raises and the row is rejected. The atomic transaction
remains the only legitimate creation path; arbitrary SQL writes
that try to bypass it now hit a DB-level wall.

#### Capabilities matrix

| Capability | `kind='personal'`, default (`default_for_user` set) | `kind='personal'`, secondary (`default_for_user IS NULL`) | `kind='shared'` |
|---|---|---|---|
| Membership shape | exactly 1 user-owner row | exactly 1 user-owner row | 0..N users + 0..N groups at any role |
| `add_member` | refused | refused | allowed |
| `remove_member` | refused (sole owner is fixed) | refused (sole owner is fixed) | allowed except sole owner |
| Rename | allowed (by the owner) | allowed (by the owner) | allowed (by any owner) |
| Delete via API | **refused** — deleting this loses all the user's files; the only path is user-delete cascade | allowed (it's just a silo) | allowed (by an owner; CASCADEs the drive's contents) |
| Default-drive lookup result | this drive | never | never |
| On user-delete | `ON DELETE CASCADE` via `default_for_user` FK (free) | application-layer cleanup: enumerate via `role_grants` (`subject_id=<user> AND resource_type='drive' AND role='owner'`) and delete | role_grants rows referencing the user are dropped; refuse user-delete if any shared drive would lose its last owner |
| Group ownership | no | no | yes |
| Per-resource grant outward | yes (subject to drive policies) | yes | yes |
| Cross-drive move | yes (subject to `forbid_cross_drive_move`) | yes | yes |
| Kind conversion | no — always default-personal | yes → may be promoted to `kind='shared'` later (drops the single-user restriction, picks up members) | no |
| Change `quota_bytes` | N/A — `drives.quota_bytes` is NULL for personal drives. The envelope is `auth.users.storage_quota_bytes` (admin-only — §7) | N/A — same | **OxiCloud admin only** (not the drive owner — §7) |

### 4. Roles → permission bundles

Drive-level roles map to existing ReBAC `Permission` values via union
expansion:

| Role | Permissions implied on every resource inside the drive |
|---|---|
| `viewer` | `Read` |
| `editor` | `Read`, `Create`, `Update`, `Comment` |
| `owner`  | `Read`, `Create`, `Update`, `Comment`, `Delete`, `Share`, *and* drive-level admin (rename, manage non-Owner members). **Policy mutation and quota mutation are OxiCloud-admin only (§7, §8)** — owners cannot self-grant capacity or relax compliance gates. Owner-role mutations are admin-only when `forbid_owner_role_change` is on (§8). |

### 5. Permission resolution — additive over `role_grants`

A user has permission `P` on resource `R` if any of the
`role_grants` rows reachable from them (directly, via group
membership, or via drive membership) carries a role whose bundle
includes `P`. Specifically:

- A direct grant: `(subject_type='user', subject_id=$user, resource_id=$R, role=…)`
- A group-mediated grant: any `role_grants` row whose
  `subject_type='group'` points at a group the user is a transitive
  member of.
- A drive-mediated grant: any `role_grants` row whose
  `resource_type='drive'` and `resource_id=$R.drive_id`, with
  the same per-row role bundle.

Permissions are **additive**. Drive role is the baseline floor for
every resource in the drive; explicit per-resource grants only add
on top. There is no "revoke for this file even though you're a
drive editor" concept, matching Google Workspace.

The auth engine reads `role_grants` as a single source of truth.
Cache invalidation has one keyspace (rows in `role_grants`) instead
of two. The drive-membership lookup ("what drives can this caller
read?") is a `WHERE subject_id=$1 AND resource_type='drive'` scan
against the same table.

### 6. Lifecycle rules

| Event | Behaviour |
|---|---|
| New internal user registers | Auto-create a default personal drive (`kind='personal'`, `default_for_user=<new_user>`, `quota_bytes=NULL` — the envelope lives on `auth.users.storage_quota_bytes`, see §7) + its root folder (`name='Personal'`, `parent_id=NULL`, drive_id pinned) + the Owner role_grant (`role_grants(subject_type='user', subject_id=<user>, resource_type='drive', resource_id=<drive>, role='owner')`) — **all four writes in one transaction** (§3), atomic against server crash. |
| External user invited (magic-link only) | **No personal drive created.** External users are grant-only recipients with no storage. |
| External user converts to internal (future flow) | Default personal drive created at conversion time. |
| User deleted | **Default** personal drive cascade-deletes via `ON DELETE CASCADE` on `default_for_user`. **Secondary** personal drives (`kind='personal' AND default_for_user IS NULL` and whose sole owner `role_grants` row points at the user) are deleted by an application-layer pass in the same transaction. `role_grants` rows referencing the deleted user are removed from all shared drives. If a removal would leave a shared drive with zero owners, deletion is refused — admin must transfer first. |
| Group deleted | Refuse if the group is a member of any shared drive that would lose its last owner. Admin must transfer or remove the group's role from those drives first. (Groups can't be members of personal drives.) |
| Add member to personal drive | Refuse. Personal drives are single-user — collaborate via per-resource grants or by moving content into a shared drive. |
| Remove sole owner of personal drive | Refuse. The only deletion path for a personal drive is user-deletion via cascade. |
| Delete personal drive | Refuse from the API. Only ON DELETE CASCADE (user deletion) drops it. |
| Rename personal or shared drive | Allowed for any owner-role caller. The drive's display name lives on its root folder (§3) — rename via `PATCH /api/folders/<root_folder_id>`, not a drive-specific endpoint. |
| Remove last owner of shared drive | Refuse — drive must always have ≥1 owner. App-layer check on `DELETE FROM role_grants WHERE resource_type='drive' AND resource_id=$drive AND role='owner'`. |

### 7. Quota model

Two ceilings, two different jobs:

- **Per-user envelope** — `auth.users.storage_quota_bytes` stays
  as the canonical "how much can this user store on this server."
  It caps the sum of `used_bytes` across **every personal drive
  the user owns** (default + any secondaries — see §2). Shared
  drives never count against any user envelope.
- **Per-drive ceiling** — `drives.quota_bytes` is a per-drive cap
  that applies **only to shared drives**. For personal drives
  this column is `NULL` (unlimited at the drive layer); the
  effective cap comes from the user envelope.

Why the asymmetry: a user's personal storage is a single budget
that the operator has agreed to provide; splitting it into
sub-quotas per personal drive is a sub-quota UX trap (users now
have to plan how to allocate "their" bytes between drives they
own). A shared drive's quota IS the team's resource budget, owned
by the operator, set independently.

#### Upload gate

Pre-upload, both checks run, in order:

1. **Drive cap** (`drives.quota_bytes`) — skipped when NULL.
   Always skipped for personal drives by virtue of the NULL
   convention; applies for shared drives.
2. **User envelope** (`auth.users.storage_quota_bytes`) — runs
   only when the target drive is personal. The check sums
   `used_bytes` across the caller's personal drives (or, fast
   path while no secondaries exist on the UI, reads the cached
   `auth.users.storage_used_bytes`).

For shared-drive uploads, only the per-drive check applies and the
user envelope is untouched — collaborating in a 1 TB shared drive
costs no personal bytes.

#### `used_bytes` accounting

Maintained incrementally on every file insert/delete in
`storage.drives.used_bytes`. The user-side cached counter
(`auth.users.storage_used_bytes`) is updated **only when the
target drive is personal** — the per-upload delta hook reads
`drives.kind` from the same query that already fetches
`drives.used_bytes` for the drive-cap check, so the hot path adds
zero round-trips.

A periodic reconciliation job rebuilds both counters from ground
truth:

```sql
-- Per-drive: unchanged from today.
UPDATE storage.drives SET used_bytes = (
    SELECT COALESCE(SUM(size), 0) FROM storage.files
     WHERE drive_id = d.id AND NOT is_trashed
) d;

-- Per-user: sum of personal-drive used_bytes owned by the user.
UPDATE auth.users u SET storage_used_bytes = COALESCE((
    SELECT SUM(d.used_bytes)
      FROM storage.drives d
      JOIN storage.role_grants g
        ON g.resource_type = 'drive' AND g.resource_id = d.id
       AND g.role = 'owner'
       AND g.subject_type = 'user' AND g.subject_id = u.id
     WHERE d.kind = 'personal'
), 0);
```

Fast-path variant while only default personals are exposed:

```sql
UPDATE auth.users u SET storage_used_bytes = COALESCE((
    SELECT used_bytes FROM storage.drives WHERE default_for_user = u.id
), 0);
```

Reconciliation runs on the maintenance pool — never blocks
uploads. Drift between deltas and the sweep is bounded by the
sweep interval (default 10 min).

#### Quota mutation is OxiCloud-admin only

Changing `drives.quota_bytes` (shared drives only) is **not** in
the drive `owner` role bundle (§4). It requires the tenant-level
OxiCloud admin role (`auth.users.role = 'admin'`), checked at
`PATCH /api/admin/drives/{id}/quota`. Drive owners can rename,
edit policies, and manage members; they cannot self-grant
capacity.

Changing `auth.users.storage_quota_bytes` (the personal envelope)
is likewise admin-only — same surface and audit pattern as today.

Why this seam matters:

- **Resource allocation is a tenant concern.** Storage bytes are
  a finite system resource the operator pays for. The drive owner
  is empowered over the drive's *use*; the admin is empowered
  over its *budget*.
- **Privilege-escalation seam closed.** Without the per-drive
  carve-out, an Owner of a shared drive could raise its quota.
  Without the per-user carve-out, any internal user could raise
  their own envelope by virtue of owning their personal drive.
- **Shared-drive coherence.** A shared drive's quota is set by
  the operator at provisioning; subsequent capacity requests go
  through the admin, not the drive's group owners.

Audit log emits `drive.quota_changed` (shared drives) and
`user.quota_changed` (envelope) with `granted_by=<admin_user_id>`
and the old/new values.

#### Multiple personal drives — schema-ready, no public surface

The schema and service layer treat personal drives as "any
personal drive owned by a user counts against the envelope," so
secondary personal drives (`kind='personal' AND
default_for_user IS NULL`) just work the day they ship. Today
there is **no public API surface to create them** — the only
`POST /api/drives` flow creates shared drives, and personal-drive
provisioning happens at user registration via the lifecycle hook
(§6). The capability matrix (§3) keeps the secondary column for
the migration backfill path and for the future, but it is not
user-reachable.

When secondary personals are eventually exposed (e.g. a "Vault"
end-to-end-encrypted drive kind, or a "Work" silo with a stricter
policy bag), the quota model needs no change — the sum-of-personal
formula already accounts for them.

#### Chunk dedup vs per-drive quota

With the CDC chunk store landed in v0.7.0 (see
`delta_upload_service`, `upload_ingest`, instant upload by hash),
a single chunk can be referenced by files in multiple drives. The
accounting decision: **each drive counts the file's logical size
in full against its own `used_bytes`** — dedup savings are
server-side only and never visible in the per-drive quota number.
This matches the existing per-user blob-dedup model and avoids
the "pro-rated quota" trap (which makes quota math depend on
cross-drive content and breaks the user's mental model of "I have
1 TB free"). Reconciliation sums file sizes per drive, not chunk
allocations.

#### Migration

One-shot at deploy: NULL out `drives.quota_bytes` for every
`kind='personal'` row (D4 backfilled them from
`auth.users.storage_quota_bytes` for the original "every drive
owns its quota" plan). Then run the new reconciliation sweep once
to resync `auth.users.storage_used_bytes` to "sum of personal
drives" (excludes any shared-drive bytes the old delta path may
have charged to it). Both steps idempotent.

### 8. Policies (JSONB, extensible)

Each drive carries a `policies` JSON object. Six known keys for v1:

```jsonc
{
    "forbid_sharing":           false,  // disables per-resource grants on this drive
    "forbid_external_sharing":  false,  // blocks grants to is_external=true subjects
    "forbid_public_links":      false,  // blocks token-share (anonymous link) creation
    "forbid_cross_drive_move":  false,  // blocks MOVE when src.drive_id != dst.drive_id
    "forbid_owner_role_change": false,  // locks the Owner roster against non-admin callers
    "read_only":                false   // full freeze — every mutation refused (user + background)
}
```

#### Mutation: OxiCloud-admin only

`PATCH /api/drives/{id}/policies` is **OxiCloud-admin only** — the
same carve-out that guards `drives.quota_bytes` and
`users.storage_quota_bytes` (§7). The original design had policies
owner-mutable, but that made them **self-policing soft caps**: an
owner could disable `forbid_external_sharing`, mint the grant, and
re-enable the policy. The audit log would capture the toggle but
the policy gave no compliance-grade enforcement.

Restricting mutation to the tenant operator closes that hole. Drive
owners can still see the current policy values via
`GET /api/drives` (read-only) but can't flip them; a UI surface that
submits an admin ticket handles the self-service case for
single-owner shadow.tech-style deployments.

Anti-enumeration: non-admin callers receive `404` on the PATCH (the
same response a non-existent drive would carry), never `403`, so a
probe can't tell the policy state apart from the drive's existence.

Enforcement points (one place per policy — single grep target):

| Policy | Enforcement callsite |
|---|---|
| `forbid_sharing` | `grant_handler::create_grant` — checks `resource.drive_id`'s policy before insertion |
| `forbid_external_sharing` | `grant_handler::create_grant` (early Email + late User checks for File/Folder) and `DriveManagementService::set_member_role` (Drive resource + the membership endpoints) |
| `forbid_public_links` | `share_service::create_shared_link` and `grant_handler::create_grant` (when subject is `Token`) |
| `forbid_cross_drive_move` | `file_management_service::move_file_with_perms` and `folder_service::move_folder_with_perms` — refuse when `src.drive_id != dst.drive_id` |
| `forbid_owner_role_change` | `DriveManagementService::set_member_role` (refuses Owner-role writes + demotions of current Owners) and `::remove_member` (refuses removals of Owners) — non-admin callers only |
| `read_only` | `PgAclEngine::check_inner` — every permission except `Read` is refused on File/Folder/Drive resources in the drive (compliance-grade freeze). Background trash-retention purge (`trash_db_repository::delete_expired_bulk`) filters out read-only drives at SELECT time so the JVM-side gate has a matching database-side gate: neither surface can mutate a frozen drive. Cached in `drive_policies_cache` (30 s TTL, invalidated on every policy PATCH). Admin escape hatch remains via `admin_guard` on `PATCH /api/drives/{id}/policies` — bypasses `authz.require` so admin can always un-freeze. |

Default to `false` (everything allowed). Admin opts in per drive via
`PATCH /api/drives/{id}/policies`.

#### Policy semantics — subtleties to remember

- **`forbid_sharing`** disables **per-resource** grants on resources
  in the drive. Drive owners can still add **drive-level** members
  (otherwise the drive becomes uneditable except by the original
  owner). The policy means "no fine-grained sharing of individual
  files; access happens through drive membership only".
- **`forbid_cross_drive_move`** protects against exfiltration via UI
  move. It does **not** stop download + re-upload (that's a different
  category of policy — file-egress, future). UI surface should make
  this explicit so users don't read it as data-leak protection.
- **`forbid_owner_role_change`** locks the Owner roster against
  non-admin mutation. After admin provisions the drive's owners, no
  Owner can add a co-owner, be demoted, or be removed by another
  Owner — only admin can change the roster. Editor / Viewer
  mutations by remaining owners are unaffected. Personal drives
  already refuse every member mutation via `refuse_if_personal`, so
  this policy only adds value on shared drives. Pairs naturally with
  the admin-only `PATCH /policies` carve-out above: once admin sets
  the owners + locks the policies, the configuration is genuinely
  immutable from the owner side.
- **`read_only`** is the **full freeze** — every permission except
  `Read` is refused on every resource in the drive, regardless of
  role. Legal-hold / archive / account-wind-down use case. Two
  enforcement homes on purpose:
  - **Foreground** — `PgAclEngine::check_inner` gates every mutating
    `authz.require` call. Cached in `drive_policies_cache` (subject-
    independent, 30 s TTL, invalidated on `update_policies`). Emits
    `event = "authz.denied"` with `reason = "drive_read_only"` before
    returning false, so operators can filter freeze-caused denials
    from ordinary role denials.
  - **Background** — `trash_db_repository::delete_expired_bulk` adds
    a SQL predicate `AND (d.policies->>'read_only')::boolean IS NOT
    TRUE` on both the file and folder purge branches. A tick already
    in flight is allowed to complete (option A on the freeze-mid-tick
    race — legal-hold uses set the policy *before* the compliance
    window opens, so the race isn't practical). Blob GC and orphan-
    upload sweeps are neutral by construction: they operate at the
    blob / temp-directory layer, not on drive-scoped file rows.
  - Applies to both personal and shared drives — a user winding down
    their account, freezing a secondary personal archive, and a
    shared drive on legal hold all use the same knob.
  - Admin escape hatch is unaffected: `PATCH /api/drives/{id}/policies`
    sits behind `admin_guard` at the handler layer and bypasses
    `authz.require` entirely, so admin can always un-freeze.

#### Future policy keys (out of scope for v1 — but the JSONB shape
accommodates them without schema migration)

- `timeboxed_session` — drive contents require re-auth after N
  minutes. Significant UX/middleware lift; defer.
- `end_to_end_encrypted` — client-side encryption; massive scope.

### 9. URL surface

#### Frontend

| URL | Resolves to |
|---|---|
| `/` (internal user) | Redirect to `/files/<root-folder-id>` of the caller's default personal drive |
| `/` (external user) | Redirect to `/shared-with-me` (no personal drive exists) |
| `/files` | Default browse — shows the caller's home root (back-compat) |
| `/files/<folder-id>` | Folder view at this folder. Drive context is recovered server-side from `folders.drive_id`. Switching drives = navigating to the new drive's root folder id |
| `/files/<a>/<b>/<c>` | Folder `c` (descendant of `b`, descendant of `a`). Each segment is a folder UUID; the prefix chain provides breadcrumbs without a server round-trip |
| `/config/drive/<drive-uuid>` | Drive configuration surface (members, policies, quota). Page is permission-aware: owner sees member management; editor/viewer see a read-only "Drive info" view |
| `/config/user/<user-uuid>` | (Future) User configuration — same shape so the `/config/<resource-type>/<uuid>` pattern is consistent across resources |
| `/drive/<...>` | **Reserved** for future drive-scoped surfaces that aren't covered by `/files/` or `/config/drive/` |

**Why `/files/<folder-id>` and not `/drive/<folder-id>`**: the existing files browser already takes a chain of folder UUIDs (`/files/<id1>/<id2>/<id3>`), with the leaf being the current folder and the prefix providing breadcrumbs. Switching drives just means navigating to a different root folder id under the same prefix — no new route shape required. Reserving `/drive/<...>` for later keeps the door open without forcing a migration now.

**Why folder-id, not drive-uuid + folder-id**: every `storage.folders` row carries `drive_id` after D0, so a single folder UUID recovers the drive context in one cheap lookup. Stable across cross-drive moves (D6): bookmarks keep working when a folder hops drives, because the folder UUID doesn't change.

**Why `/config/` is a separate top-level segment**, not `/drive/<uuid>/settings`: the URL prefix encodes intent ("we are configuring something"), not just resource location. Future configuration surfaces (`/config/user/<id>`, `/config/group/<id>`, `/config/share/<id>`) compose cleanly under the same prefix. It also avoids the singular-vs-plural ambiguity (`/drive/<X>` vs `/drives/<X>/settings`) that's easy to typo and hard to grep for.

#### Native WebDAV (`/webdav/...`)

**SHIPPED 2026-07-06.** Config-driven via env
`OXICLOUD_WEBDAV_DRIVE_LISTING_PREFIX` (`FeaturesConfig::webdav_drive_listing_prefix`;
default `"@drive"`, sanitized by trimming leading/trailing `/`).
Three deployment shapes:

| `WEBDAV_DRIVE_LISTING_PREFIX` | URL | Resolves to |
|---|---|---|
| `@drive` (default) | `/webdav/…` | caller's default personal drive (back-compat) |
| `@drive` | `/webdav/@drive/` | drive listing |
| `@drive` | `/webdav/@drive/<sel>/…` | specific drive |
| `""` (empty) | `/webdav/` | drive listing |
| `""` | `/webdav/<sel>/…` | specific drive |
| any other | same shape as `@drive`, segment substituted | |

`<sel>` is a drive UUID **or** the drive's display name (matched
against `storage.folders.name` of the drive root). Only drives the
caller has Read on via `role_grants` resolve; unknown selector and
permission denial both return 404 (anti-enumeration).

**Why the `@drive` sigil and NOT `/webdav/drives/<uuid>/...`**
(earlier draft) or top-level `/drives/<uuid>/...` (also
considered): `@` is the established structural-routing sigil
(GitHub `@user/repo`, npm `@scope/pkg`, LDAP `@domain`) — it
reads as "this is not user content, this is a routing token."
Realistic collision risk drops to near-zero: nobody creates a
top-level folder named exactly `@drive` by accident. Keeps **one
URL root for everything WebDAV** — single `<Location>` block in
reverse-proxy configs, single mental model for sysadmins, single
dispatcher in `webdav_routes()`. Making the segment
config-tunable per deployment lets operators pick a different
sigil (`drives`) or drop it entirely (`""` = drive-listing at
root) without a code change.

**Implementation:** `resolve_webdav_scope` in
`src/interfaces/api/handlers/webdav_handler.rs`. Selector accepts
UUIDs and display names; UUID form is tried first. Legacy
tolerance in the default-drive branch: bookmarks that already
carried the drive-root name as their first segment
(`/webdav/Personal/foo` under a Personal-default user) are
passed through instead of double-prepended.

**Hurl coverage:**
- `tests/api/webdav_drive_root.hurl` — default `@drive` config
- `tests/webdav-drive-root/drive_root_empty_config.hurl` — empty
  config (separately-configured server; runs under
  `tests/webdav-drive-root/run.sh`, wired into `just api-test`
  and CI's `api-test` job)

**Href construction — verified drive-aware:** `webdav_href()`
prints `/webdav/<path>`, but the `<path>` input is `client_path`
extracted from `req.uri()` (the URL segment after `/webdav/`), not
the scope-resolved db_path. So a request to
`/webdav/@drive/<sel>/folder/` renders children as
`/webdav/@drive/<sel>/folder/<child>/` — the `@drive/<sel>/`
prefix is preserved on every hop. `client_path` is threaded into
`base_href` at `handle_propfind` and passed through
`build_streaming_propfind_response` unchanged.

**Deferred (not blocking):**
- One-liner guard refusing folder creation named literally
  `@drive` at drive root (defensive against future collisions —
  today an unknown `@drive` folder at drive root is unreachable
  via WebDAV under the default config, so it's low priority).
- Cross-drive MOVE / COPY currently 403 — same-drive only.
  Cross-drive copy has REST-side support; WebDAV MOVE/COPY
  could route through it once permission mapping is designed.

#### NextCloud-compat WebDAV (`/remote.php/dav/...`)

> **The path-segment `/drives/<uuid>/` form is NOT used on the NC
> surface.** It is reserved for the native WebDAV surface (see
> table above). The NC surface keeps the URL shape
> `/remote.php/dav/files/<username>/<path>` and carries the drive
> selector in the **credential**, not the URL.
>
> NC desktop / mobile clients store credentials per
> `(host, username)` and offer a single sync root per saved
> account. A path-segment scheme would require NC clients to grow
> multi-root awareness, which they don't have. Two
> credential-side mechanisms are valid here:
>
> 1. **Username discriminator (`{user}~{drive-uuid}`)** — the
>    chroot POC on `feat/nextcloud-drive` (commit `137169b7`).
>    The Basic Auth username carries the drive UUID after a `~`
>    separator; the URL stays under `/remote.php/dav/files/{user}~{uuid}/<path>`.
>    Explicit on the wire, no per-credential server state needed
>    beyond the existing app-password row.
>
> 2. **App-password ↔ drive binding** — store the chosen drive
>    UUID directly on the `auth.app_passwords` row at issuance
>    time. The Basic Auth username stays as `{user}` (clean
>    NextCloud UX, no `~` to explain). The auth middleware looks
>    up the app-password row, reads its `drive_id` binding, and
>    uses that as the drive context. Each drive a user wants to
>    sync gets its own app-password.
>
> Both are workable; option (2) is the cleaner UX (username
> matches what users type, no extra character to explain) but
> requires a schema add on `auth.app_passwords` and one extra
> JOIN in the hot auth path. Option (1) is the smallest possible
> change but exposes the `~` to the user. They're not mutually
> exclusive — the issuance flow can produce credentials in either
> shape. Decide before D1 ships which is the **default** the
> Login Flow v2 picker produces.

| URL | Resolves to |
|---|---|
| `/remote.php/dav/files/<username>/<path>` | That user's personal drive — unchanged. (App-password drive-binding NULL ⇒ personal.) |
| `/remote.php/dav/files/<username>~<drive-uuid>/<path>` | Option 1: explicit drive in the URL. |
| `/remote.php/dav/files/<username>/<path>` *(with `auth.app_passwords.drive_id` set)* | Option 2: drive resolved from the credential row. URL is indistinguishable from the unchanged personal case to the client. |

In either case, the auth middleware asserts the caller is a member
of the resolved drive before serving any DAV verb. Pre-existing NC
clients pointed at `/remote.php/dav/files/<username>/` continue
syncing the user's personal drive without reconfiguration —
regardless of which option is chosen as the default.

#### Username/UUID collision — defused

The `/remote.php/dav/files/<x>/` vs `/remote.php/dav/drives/<x>/`
split solves the worry about username/UUID ambiguity. The
discriminator is the literal segment (`files` vs `drives`), never
the value of `<x>`. A user happening to have a UUID-shaped username
is no longer a problem.

### 10. Storage paths — wrapper folder becomes the drive's root folder

Today `storage.folders.path` is e.g. `My Folder - admin/Docs`. The
"My Folder - admin" wrapper is the user's home folder, created at
registration via `format!("My Folder - {}", username)`.

Post-drives, **the wrapper isn't deleted — it's *adopted* as the
drive's root folder** (§3). The drive row is created alongside it
and points at it via `drives.root_folder_id`. The wrapper's row
survives the migration; only its `name` is updated.

```
Drive (uuid=…, kind=personal, default_for_user=admin)
└── root folder (parent_id=NULL, drive_id=<drive_uuid>, name="Personal")  ← was "My Folder - admin"
    ├── Docs/
    └── aa.pdf
```

Shared drives follow the same shape — drive + root folder + content
underneath:

```
Drive (uuid=…, kind=shared, owners=group:engineering)
└── root folder (parent_id=NULL, drive_id=<drive_uuid>, name="Engineering")
    ├── Specs/
    ├── Roadmap.md
    └── archive/
```

One rule, one model: **every drive has exactly one folder where
`parent_id IS NULL` AND `drive_id = <the drive>`**.

#### Why this is a better model

Three things converge:

1. **No API duality.** Folder creation is always `POST /api/folders
   { name, parent_id: <id> }`. There's no polymorphic "create at
   drive vs in folder" branch — the drive's root folder is just
   another folder id from the client's perspective. The `parent_id`
   field that exists today carries over unchanged.
2. **No path-prefix rewrite migration.** The wrapper row stays; it's
   renamed to its drive's canonical name (`"Personal"` for the
   default, the original sibling-root name for secondaries). The
   BEFORE-UPDATE path trigger fires on the rename and the cascade
   trigger automatically rewrites every descendant's `path` /
   `lpath` — no per-row UPDATE in the migration. The net cost is
   one UPDATE per drive plus the trigger's cascade.
3. **No "this user lost their drive" failure mode.** Migration is
   safe even mid-flight — the wrapper row never disappears, just
   gains a drive_id pointer above it.

#### Why this is client-safe

The wrapper was already invisible to WebDAV / NC clients pre-drive:

- NC clients hit `/remote.php/dav/files/<user>/<path>` and
  `nc_to_internal_path` prepended `My Folder - <user>/` internally
  before talking to the storage layer. The client never saw the
  wrapper segment in its URL.
- Native `/webdav/<path>` was implicitly chrooted to the user's
  home by `resolve_webdav_path`. Same story.

Post-migration the resolver chroot becomes `<drive_root_folder.path>/`
instead of `My Folder - <user>/`. The drive's root folder name
(e.g. `Personal`) replaces the wrapper name in the materialised
`path` column; the client's URL still doesn't carry it
because the resolver still chroots before talking to storage. Net
effect on the wire: zero.

#### Uniqueness constraints become drive-scoped

Pre-drive, two indexes enforce "no duplicate folder names under the
same parent for the same user":

```sql
CREATE UNIQUE INDEX idx_folders_unique_name
    ON storage.folders(parent_id, name, user_id)
    WHERE NOT is_trashed AND parent_id IS NOT NULL;
CREATE UNIQUE INDEX idx_folders_unique_name_root
    ON storage.folders(name, user_id)
    WHERE NOT is_trashed AND parent_id IS NULL;
```

Both move from `user_id`-scoped to `drive_id`-scoped:

```sql
CREATE UNIQUE INDEX idx_folders_unique_name
    ON storage.folders(parent_id, name, drive_id)
    WHERE NOT is_trashed AND parent_id IS NOT NULL;
CREATE UNIQUE INDEX idx_folders_unique_name_root
    ON storage.folders(name, drive_id)
    WHERE NOT is_trashed AND parent_id IS NULL;
```

This is a **correctness improvement**, not just a migration concession.
The semantic users expect is "no duplicate names *within a drive*"
— a folder named "Reports" in your Personal drive shouldn't preclude
another "Reports" in a shared "Team" drive. The user-scoped
constraint forbade that. The drive-scoped constraint allows it.

For the root variant: post-migration each drive has exactly one
`parent_id IS NULL` row (its root folder), so `(name, drive_id)` is
trivially unique. The index is still worth keeping as
defence-in-depth.

#### Sibling root folders also become drives

Today the only way to get a `parent_id IS NULL` folder is via the
home-folder creation path, which produces exactly one per user.
However, manual SQL has already been used (and may still be used
by ops) to create additional top-level folders — folders that sit
beside `My Folder - <user>` at `parent_id IS NULL`. These are
**not** addressable today through any handler (the UI assumes
exactly one root); they exist as data only.

Post-migration, the model has to absorb them. Rule:

- For each user, find every row with `parent_id IS NULL AND user_id
  = <this user>`.
- The one named `My Folder - <username>` becomes the **default
  personal drive's root folder**: a new drive row is created with
  `kind='personal'`, `default_for_user=<user>`, the existing folder
  row gets a `drive_id` pointer (and the wrapper's name is updated
  to `"Personal"`), and `drives.root_folder_id` points back at it.
  Sole `role_grants` row: Owner, subject=`<user>`.
- Every **other** sibling becomes a fresh **secondary personal
  drive's root folder**: a new drive row with `kind='personal'`,
  `default_for_user=NULL`, the existing folder row gets its
  `drive_id` set and *keeps its name* (`Archive`, `2024 Projects`,
  whatever), quota initialised from the user's quota
  (`auth.users.storage_quota_bytes`) — same default as the user's
  primary personal drive. Membership rules from §2 apply: the user
  cannot invite co-owners while the drive remains personal. To open
  the silo up, the user can later **convert** the secondary
  personal to `kind='shared'` (an application-layer operation that
  flips the kind and lifts the single-user restriction so the
  membership API can add other users / groups).
- **No folder row is deleted.** The wrapper rows survive the
  migration as drive root folders; only their `drive_id` is set and
  (for the default-Personal case) their `name` is updated.

The chroot POC's "pick a drive at login" picker on
`feat/nextcloud-drive` already produces the right shape for this:
users with one drive auto-select Personal silently, users with
N drives get a real picker. No POC change needed — it just sees
real drive rows instead of folder UUIDs.

### 11. Content search index — cross-drive with anti-enum filtering

v0.7.0 added an embedded Tantivy full-text content index (see
`infrastructure/services/search_index/tantivy_content_index.rs`
and migration `20260701000000_content_search_index.sql`). Today
every indexed document carries the owning user as a filter field
and queries restrict by that field at query time.

The pivot to drives keeps **one global endpoint, `/api/search`,
that aggregates across every drive the caller can read**. There
is no per-drive search route in D0; the URL/picker UI in D1 may
add an optional `?drive_id=<uuid>` narrowing parameter, but the
default surface stays cross-drive.

The security primitive that makes cross-drive search safe is the
**`Occur::Must` clause applied at query time**, not after.
Tantivy's collector only sees documents that satisfy the Must
clause — stored fields (`preview`), counts, and pagination
cursors all reflect the filtered set. This is the same shape the
existing `user_id` filter uses today; we keep it and swap the
field.

1. **Schema update**: every indexed document gains a `drive_id`
   STRING field stamped at ingest time. The existing `user_id`
   field is kept during the dual-write window for rollback
   safety and dropped in D7.
2. **Query path**: replace the `Must user_id = caller` clause
   with a `Must drive_id ∈ accessible_drives` set-membership
   clause. `accessible_drives` is computed fresh per query
   (personal + every shared-drive membership), reusing the
   `expand_user` cache that `PgAclEngine` already maintains and
   `AuthzCacheLifecycleHook` already invalidates on membership
   change.
3. **Handler-side ReBAC re-verification** (defense in depth):
   after Tantivy returns hits, the `/api/search` handler
   re-checks each `file_id` with the engine. The re-check is
   *subtractive only* — it can drop a Tantivy hit, never add
   one. It catches:
   - **Index staleness** — file just moved to a drive the caller
     can't access; the indexer hasn't caught up so Tantivy still
     returns the doc under its old drive_id (a false positive
     from the caller's perspective). The re-check denies and the
     hit drops.

   #### Known D0 scope limit — per-resource grants invisible in search

   The drive-only Must clause makes the inverse case structurally
   invisible: a file or folder shared *directly* with the caller
   via ReBAC, living in a drive the caller has no membership in,
   never reaches the re-check because Tantivy already filtered the
   doc out at index-query time. Concretely:

   - Alice grants Bob a per-file Read on `report.pdf` inside her
     Personal drive. Bob has no drive grant on Alice's Personal.
   - Bob's `accessible_drives` set doesn't include Alice's drive.
   - Tantivy's `Must drive_id ∈ accessible_drives` rejects every
     doc with Alice's drive_id, including `report.pdf`.
   - Bob's search misses `report.pdf` even though `authz.check(Bob,
     Read, File(report))` would say yes.

   This is **not a security issue** — Bob still can't access
   anything he isn't entitled to. The trade-off is *discovery
   only*: search doesn't surface directly-shared resources. The
   "Shared with me" UI is the canonical surface for that workflow
   (resources someone explicitly shared with you live in the
   notification / inbox / share-listing flow, not in global search).

   Closing this gap is deferred. Two future moves, in order of cost:
   - **Per-file grants** (low cost): one extra `role_grants` query
     for `resource_type='file' AND subject ∈ caller_set`, widen
     the Must clause to `drive_id ∈ A OR file_id ∈ F`. ~1 day of
     work; deliverable when the "Shared with me" search surface is
     prioritised.
   - **Per-folder grants with cascade** (high cost): the ltree
     subtree expansion is what makes it gnarly — a grant on folder
     `F` should surface every descendant in search. Two viable
     shapes: (a) reindex with a multi-valued `ancestor_folder_ids`
     STRING field on each doc (schema v3, full reindex), or (b)
     expand each granted folder to its subtree at query time
     (per-grant recursive SQL on the hot path). Probably a dedicated
     PR alongside the D1 URL-routing work.
4. **Token subjects cannot search**. `/api/search` returns 401
   for token-authenticated callers (anonymous link tokens have
   access to one resource, not a drive — there is no meaningful
   "search my drives" surface for them). The 401 is consistent;
   "empty results" would leak nothing but would be operationally
   confusing.
5. **Anti-enumeration response shape**: no "you have N hidden
   matches" anywhere. The count, the cursor, and the snippet
   list all reflect the filtered set and nothing else.
6. **Treat the reindex as a blocking step of D0**, not a D4/D5-
   era polish item — otherwise the index is the silent leak
   path during the dual-write window. The reindex re-runs the
   existing `content_index_worker::drain_once()` loop after
   `TantivyContentIndex::open_or_rebuild()` detects the schema
   version bump; no separate CLI command needed (per the audit).

Hurl coverage in D0 includes a concrete anti-enumeration test:
user A indexes a file in a drive user B can't see, user B searches
the indexed term, response is empty + no hidden-count leak.

Filtering on a low-cardinality `drive_id` is something Tantivy
handles natively; this is bookkeeping, not a query-plan risk.

### 12. Trash — per-drive scoped, owner-actioned

Today trash is per-user: `storage.files` / `storage.folders`
carry `is_trashed BOOLEAN` + `trashed_at` + `original_parent_id`
(soft-delete in place), and `storage.trash_items` is a VIEW that
UNIONs them. The listing endpoint (`GET /api/trash/resources`)
filters by `user_id = caller`.

Post-drives, trash becomes **per-drive**:

- **Storage shape is unchanged.** The `drive_id` column added to
  `storage.files` / `storage.folders` in Phase A already
  identifies which drive a trashed row belongs to. No new
  trash table, no schema work beyond updating the
  `storage.trash_items` VIEW to surface `drive_id` alongside
  (or replacing) `user_id`.
- **Trash listing query** filters by drive(s) the caller can
  read. Default listing returns trash from every drive the
  caller has membership on; a `?drive_id=<uuid>` parameter
  scopes to one drive. UI shows a drive picker above the trash
  list, same as the main file view.
- **Trash mutations are owner-only.** Per the §4 role-bundle,
  `Delete` is in the owner bundle only — so today's "anyone
  who can delete the original can act on its trash entry" is
  already drive-owner-scoped. Specifically:
  - **Send to trash** — any drive owner (carries `Delete`).
    Personal drive: the user themselves.
  - **Restore** — any drive owner. Operation reverses
    `is_trashed`, sets `parent_id` back to `original_parent_id`
    when that ancestor is still in the same drive (otherwise to
    the drive root with a name conflict resolver).
  - **Permanent delete** — any drive owner. Clears the row and
    decrements drive `used_bytes`.
  - **View trash** — any drive member (viewer / editor /
    owner). Viewers can see what was deleted from a drive they
    have access to; only owners can act on it. Mirrors Google
    Drive's per-shared-drive trash UX.
- **Cross-drive moves carry their trash home with them**:
  when a file moves from drive A → drive B and is later
  trashed, the row's `drive_id` is B's, so trash for B sees
  it (not A's, which is the natural and expected answer).
- **Cascade on drive deletion**: when a shared drive is
  deleted (D3 will land the delete-drive flow), every
  `storage.files` / `storage.folders` row with that
  `drive_id` cascades, trashed or not. There's no need to
  "drain the trash first" — the whole drive disappears in
  one CASCADE.
- **Personal-drive trash** follows the same model: bound to
  the personal drive, sole owner (= the user) does
  everything. No new UX divergence between personal and
  shared.

The orphan/aborted-upload sweep introduced in v0.7.0
(`944c8337`, periodic trash job) must become drive-aware so
it doesn't accidentally sweep across drives the caller
shouldn't see. It already keys off ownership; the rewrite is
a per-drive pass instead of per-user.

### 13. Upload paths and quota timing

Four upload protocols, each with different "when do we know the
size" and "when do we know the destination" answers. The Drive
migration pivots quota from user-scoped to drive-scoped without
changing protocol shapes — but one path (NC chunked) carries a
pre-existing wart that the chroot POC's `~` username (or the
`app_passwords.drive_id` binding) lets us finally fix.

| Protocol | Size known | Quota check fires | Destination / drive known |
|---|---|---|---|
| Default multipart (`POST /api/files/upload`) | At request start (Content-Length / multipart `size`). | `file_upload_service.rs:185` — `check_storage_quota(caller_id, metadata.size)` before any bytes are stored. | At request start (form field `folder_id`). Drive derives from `folder.drive_id`. |
| Native chunked (`POST /api/uploads`) | At session create — client declares `total_size` in JSON. | `chunked_upload_handler.rs:213` — at session creation against declared `total_size`. | At session creation (`folder_id` in JSON). Drive derives from `folder.drive_id`. |
| **NextCloud chunked** (`/remote.php/dav/uploads/{user}/{session}/...`) | Never declared. MKCOL creates empty session, PUT chunks arrive one at a time, client decides "done". | **Today: only at the final MOVE (assemble)** — `handle_assemble` → `file_upload_service::ingest_stream_to_cas` → quota check on the assembled size. Wasted-bandwidth wart: a client over quota can upload GB before the server can refuse. | **Today**: only at MOVE (parsed from the `Destination:` header). **With the chroot POC** (`{user}~{drive-uuid}` username, see §9): known at MKCOL — the auth middleware already split the username. **With `app_passwords.drive_id` binding**: known at MKCOL — the credential row pins the drive. |
| Delta protocol (`/api/files/delta/*`) | At `negotiate` (client provides manifest with `total_size`). | `delta_upload_service.rs:331` — at commit against `total_size`. | At negotiate (target file_id or `folder_id`). Drive derives accordingly. |

#### Decision — per-chunk incremental quota check on the NC chunked path

The three non-NC paths trivially pivot to drive-scoped quota:
replace `check_storage_quota(caller_id, size)` with
`check_drive_quota(drive_id, size)`. Destination is known at
handler entry; drive falls out of the destination's `drive_id`
column. No protocol change.

For NC chunked, the Drive migration **also closes the
wasted-bandwidth wart** because the drive identity is now known
at MKCOL (via `~` username or app-password binding — either NC
credential-side scheme from §9 surfaces it). Approach:

1. **MKCOL guard** — if `drive.used_bytes >= drive.quota_bytes`,
   refuse the session creation with `507 Insufficient Storage`.
   No point letting the client even start.
2. **Per-chunk PUT check** — track cumulative bytes received in
   the session (sum of on-disk chunk sizes, maintained by the
   chunked-uploads service). On each PUT, before writing the
   chunk:
   ```text
   if drive.used_bytes + session.bytes_so_far + chunk.size > drive.quota_bytes:
       refuse with 507 Insufficient Storage
   ```
   The first chunk that would push us over is refused; client
   sees the error within one chunk's worth of wasted upload
   (typically a few MB) instead of after the whole multi-GB file.
3. **Assemble-time re-check** stays as a defence-in-depth (in
   case two concurrent sessions on the same drive each got past
   the per-chunk check but their sum exceeds quota at MOVE). This
   matches today's structure.
4. **Unlimited quota** (`quota_bytes IS NULL`) short-circuits all
   three checks — no work.

The per-chunk check is O(1) amortised: each session tracks its
cumulative size as it goes. The drive's `used_bytes` is read from
the row once per chunk; with the v0.7.0 incremental-update
pattern (`b5b80549`, `d6987329`) that's a single primary-key
lookup, not an aggregate query.

Net effect on NC clients: nothing changes for in-quota uploads;
over-quota clients get a clear 507 within seconds instead of
after the whole upload finishes.

#### Editor-role delete and trash — call out the UX tension

§4's role bundle gives `Delete` only to `owner`. That means
in a shared drive, an editor who uploads a typo file CANNOT
send it to trash themselves — they have to ask an owner. This
is already flagged as Open Question 4 (revisit before D2) and
the answer there determines the trash UX for editors too. If
editors get a `Delete` capability (or a dedicated "trash own
content" capability), the trash mutation rules become "trash
your own files" + "drive owners can act on anyone's trashed
files". Until that's decided, the conservative answer above
(owner-only mutations) holds.

### 14. File provenance — `created_by` / `updated_by`

Today `storage.files.user_id` (and `storage.folders.user_id`)
quietly doubles as **both** the ownership pointer (whose files are
these?) AND the provenance signal (who created this?). The Drive
migration cleanly separates ownership onto `drive_id`, but
provenance has to come with us — in a shared drive where Ed,
Alice, and Bob all upload files, "who put this here?" remains a
load-bearing question for UI, audit, and account-cleanup workflows.

The split:

```sql
-- on both storage.folders and storage.files:
drive_id     uuid NOT NULL                       -- ownership
created_by   uuid NULL FK → auth.users(id) ON DELETE SET NULL
created_at   timestamptz NOT NULL DEFAULT now()  -- existing
updated_by   uuid NULL FK → auth.users(id) ON DELETE SET NULL
updated_at   timestamptz NOT NULL DEFAULT now()  -- existing
```

- **`created_by`** is set once at row insert (every upload path:
  multipart, native chunked, NC chunked, streaming CDC, delta,
  instant-upload-by-hash) — the caller_id stamps it. Never
  changes after.
- **`updated_by`** is set whenever `updated_at` is touched —
  rename, overwrite, move, restore from trash, PROPPATCH on
  favorites. Same write-path discipline as `updated_at` today.
- **`ON DELETE SET NULL`** on both FKs. When a user is deleted,
  files they created or edited in shared drives stay (they belong
  to the drive, not the deleter); the FK nulls out, the UI renders
  "Unknown user" (or "Deleted user" with a small tombstone table —
  future polish, see Open Question 12).

#### Why this lands in D0, not D7

If we waited until D7 to introduce these columns, every existing
file/folder row would have NULL `created_by` after the migration —
provenance permanently lost for pre-D7 content. Adding the columns
in D0 and **backfilling from `user_id`** during the dual-write
phase gives every existing row a real `created_by` value (the user
we know created it, because that's what `user_id` meant). New
writes during the dual-write window populate both `user_id` AND
`created_by` / `updated_by`. By D7 the columns are self-sufficient
and dropping `user_id` loses nothing.

#### UI display semantics

- File details panel: "Created by Alice on 2026-01-15", "Last
  edited by Bob 3 hours ago" — both fields drive a normal user
  avatar + name lookup. NULL → "Unknown user" placeholder.
- Activity log on a shared drive: aggregates `updated_by` over
  recent rows to show "who's been active here lately".
- Account-cleanup workflow: when an admin deletes a user, show a
  pre-flight summary "Bob authored 47 files and last edited 12
  more across drives X, Y, Z — proceeding will mark those entries
  as authored by 'Deleted user'." Lets the admin choose to
  reassign or simply confirm.

#### Engine touch-point

The "who touched updated_at" rule applies uniformly across all
mutation paths. Centralise it: every service that bumps
`updated_at` must also set `updated_by = caller_id` in the same
SQL statement. The `FileRepository::update_*` and
`FolderRepository::update_*` methods are the natural choke points
— a single audit reveals every callsite to verify.

The async `tree_etag_queue` (v0.7.0) propagates an etag up the
ancestry but **does NOT** propagate `updated_by` — ETags are
fingerprints of structure, not authorship. Only the direct
mutation site updates `updated_by`.

### 15. Global sections — scope mapping

"Global sections" are the user-facing views that aggregate across
the filesystem rather than browsing a single folder: Photos, Music
library, Favorites, Recent items, Search, Trash. With drives
landing, each of these needs an explicit scope decision. The
table below locks the choices.

| Section | Scope | Capability flag (per-drive policy) | Why |
|---|---|---|---|
| **Photos** (`/api/photos`) | Default Personal Drive only | `policies.include_in_photo_index = true` to opt a non-default drive in | Non-default drives often carry images that aren't "photos" (screenshots, scans, charts-as-PNGs). Defaulting cross-drive pollutes the personal timeline. Opt-in for non-default drives where the owner explicitly wants them indexed (e.g. "Family Photos" shared drive). |
| **Music** — library view (future) + playlists | Default Personal Drive only | `policies.include_in_music_index = true` to opt a non-default drive in | Symmetric with Photos: audio files in a work drive or a random shared folder shouldn't silently bleed into the personal music library. Owner opts a non-default drive in (e.g. "Family Music", "Band Collaboration") when the drive genuinely is a music library. The Music section today is *only* playlists; a `/api/music/tracks` library view added later inherits this scope. |
| **Music playlists** (`audio.playlists`) | User-scoped, cross-drive curation | n/a | Playlists are a curation tool. `owner_id` stays on `auth.users(id)`; tracks reference files via `playlist_items.file_id` and may live in any drive the user has access to. At list time, `list_playlist_tracks` filters out tracks in drives the caller can no longer reach (see §11's defense-in-depth pattern). |
| **Favorites** (`/api/favorites/resources`) | Cross-drive (all accessible drives) | n/a | Personal organisation tool. Star a PDF from the work drive AND a photo from Personal — the whole point is cross-drive curation. ReBAC visibility check at list time drops rows the user can no longer reach. |
| **Recent items** (`/api/recent/*`) | Cross-drive (all accessible drives) | n/a | Personal history. Same shape as Favorites — you touched files across drives; the timeline reflects that. ReBAC visibility check at list time. |
| **Search** (`/api/search`) | Cross-drive (all accessible drives) | n/a | Discovery tool. See §11 for the Must-clause filter + handler-side ReBAC re-verification + anti-enum response shape. |
| **Trash** (`/api/trash/resources`) | Per-drive (owner-actioned) | n/a | Already specified in §12 — trash listing filters by drive(s) the caller can read; mutations require the owner role on the drive. |

#### Capability flag mechanism

Both `policies.include_in_photo_index` and
`policies.include_in_music_index` live under the same JSONB
`policies` column on `storage.drives` (see §8) — no new schema.
Both flags follow the same shape: **omitted = off**. The query
predicate then reduces to a single positive rule for every
drive:

```sql
WHERE fi.drive_id IN (
  SELECT d.id FROM storage.drives d
    JOIN storage.role_grants rg
      ON rg.resource_type='drive' AND rg.resource_id=d.id
   WHERE rg.subject_id IN (caller's effective subjects)
     AND (d.policies->>'include_in_photo_index')::boolean = true
)
```

No `default_for_user` OR-branch, no per-kind carve-out.

**Default personal drive gets both flags set to `true` on
creation.** The `PersonalDriveLifecycleHook` (§3) that creates
the default personal drive on user provisioning populates
`policies` with `{"include_in_photo_index": true,
"include_in_music_index": true}`. Existing default personal
drives get the same two flags via a one-shot backfill migration
alongside the flag introduction. Net effect: every user's
default personal drive is in scope from moment one, no user
configuration required for the common case, but the SQL is
kind-agnostic.

**Non-default drives** (secondary personals, shared drives) are
created with the flags omitted, so they stay out of scope until
the owner explicitly opts in via the admin "Manage policies"
modal.

Flipping either flag on any drive is instant — the query reads
`policies` at request time; the index itself is unchanged.
Toggle-off on a default personal drive is *possible* (admins
own the drive-policy mutation surface — see §8) but shows a
confirm dialog in the UI ("this will empty the user's Photos
timeline" / "…their Music library"), since it's an unusual
action.

#### Why symmetric (both opt-in) instead of asymmetric

An earlier version of this section had Music default to
cross-drive (`forbid_music_index` as an opt-out), on the
argument that audio in shared drives is "almost always
intentional content." That asymmetry created two problems:

1. **Mixed-form flag naming** — one `include_in_*` and one
   `forbid_*` with opposite meanings, hard to reason about in the
   admin UI and the query layer.
2. **The "shared audio is always intentional" claim doesn't
   hold under scrutiny** — a work drive with a few voicemail
   MP3s or a project drive with a stray podcast recording
   shouldn't bleed into the personal music library any more than
   a work drive with screenshots should bleed into Photos.

Symmetric opt-in (`include_in_*_index` for both) fixes both.
The "Family Music" case still works — the owner flips the flag
once on drive creation, same one-time gesture as "Family Photos"
under the pre-existing photo policy. The default-personal case
(90%+ of users) needs no configuration for either surface.

#### Face indexing — per-drive clustering, scope follows Photos

Face indexing is bound to the same scope as `/api/photos` — the
two surfaces show the same content set, so the face data behind
that content lives in the same scope.

Two layers to keep distinct:

**Storage layer — per blob.** Face fingerprints are keyed on
`blob_hash` (BLAKE3), FK to `storage.blobs.hash`. Fingerprints
are deterministic from content bytes, and OxiCloud dedups
content via blob hash — so a photo uploaded into N drives (or N
times by N users) produces *one* fingerprint set, computed once,
reused forever. Cascade-deletes when the blob is GC'd (ref_count
→ 0). No `user_id`, `created_by`, `file_id`, `drive_id`, or
group key on the fingerprint row: identity is the content.

**Clustering layer — per drive.** Cluster computation runs
*within* a drive: take every fingerprint reachable via a file in
that drive (`storage.files.drive_id = X` JOIN
`face_fingerprints` ON `blob_hash`), cluster them, emit clusters
scoped to drive X. The query repeats per drive the caller can
see (default personal + drives where
`policies.include_in_photo_index = true` AND the caller has
Read). Same-person fingerprints from different drives land in
**separate** clusters by default — even when both drives reach
the exact same blob, because clustering is keyed on drive, not
on fingerprint identity.

**Why per-drive clustering:**

The drive is already the data boundary post-D6 — quota, sharing,
trash, AuthZ all pivot on `drive_id`. The face library is part
of the drive's content, not a cross-drive aggregate. Two
properties fall out cleanly:

- **Family-drive UX works.** Alice and Bob both members of
  "Family" with `include_in_photo_index=true`. Alice uploads
  Christmas photos; Bob uploads birthday photos. Grandma is in
  both. Both see the *same* Grandma cluster in Family — one
  merged cluster derived from fingerprints across both uploads.
  Labels on the Family cluster are drive-scoped (anyone with
  Photos access to Family sees them).
- **Personal-drive isolation is preserved.** Each user's
  personal drive is access-isolated by definition (nobody else
  has Read on it). So a personal-drive cluster is visible only
  to the drive's owner. The privacy guarantee falls out of
  drive-access scoping — no separate user-id key needed.

**Cross-drive clusters don't auto-merge.** Bob labelling
"Grandma" in his Personal-drive cluster does NOT propagate to
Family's Grandma cluster. Two separate visual clusters by
default — even if the embedding similarity would otherwise
match them. Rationale: auto-propagating private labels into a
shared drive would silently expose personal classifications.
Future UX can offer explicit per-cluster merging ("these two
clusters are the same person") — user-driven, never silent.

**Shared-drive opt-in is the consent surface.** Enabling
`include_in_photo_index` on a drive is the owner saying "the
photos in this drive are part of the drive's photo library,
including the face data they contain." Doesn't add a new
sharing surface — surfaces what was already visible (anyone
with Read on a photo can see who's in it).

**Implementation:**

- `face_fingerprints(blob_hash, embedding, …)` — FK to
  `storage.blobs.hash`, no `user_id` / `file_id` / `drive_id`
  column. Cascade-delete via the blob ref-count → 0 GC path.
- Cluster query: `SELECT … FROM storage.files f JOIN
  face_fingerprints fp ON fp.blob_hash = f.blob_hash WHERE
  f.drive_id = $1 AND NOT f.is_trashed` for each drive in the
  caller's Photos-scope set.
- Pre-D7 the legacy `(user_id, blob_hash)` query in
  `face_indexing_service.rs::lookup_user` stays in place; D7
  drops `user_id` from the column set in lockstep with the
  global user_id retirement, leaving the fingerprint row keyed
  on `blob_hash` alone. Both the `include_in_photo_index` policy
  AND D7's user_id drop must land before face indexing can move
  to the per-drive clustering model.

#### Verification sketch

The D0 Hurl suite (`tests/api/drives_foundation.hurl`) covers
the scope decisions concretely:

- Photos: file uploaded in Personal appears in `/api/photos`; same
  file uploaded into a secondary personal drive does NOT appear
  unless `include_in_photo_index` is set on that drive.
- Music: track uploaded in any accessible drive appears in the
  library / sweeper output; setting `forbid_music_index` on a
  drive removes its tracks from the next library response.
- Favorites: star a file in drive A and a file in drive B (both
  accessible to caller); list returns both. Lose access to drive
  B → next list omits the B file (no error, just absent).
- Search: see §11 anti-enum test.

## Migration strategy

A drive-id column on every resource is a database surgery touching
every storage query. We phase it for safety:

### Phase A — additive (PR D0)

1. Create `storage.drives` (no `name` column — see §3; has
   `root_folder_id uuid NULL` populated in step 3). **No
   `storage.drive_members` table** — membership lives in
   `storage.role_grants` (created in D-Prep) as
   `resource_type='drive'` rows.
2. Add `drive_id uuid NULL` to `storage.folders` and `storage.files`.
3. **Per-user root-folder adoption sweep**: for each internal user,
   list every `storage.folders` row where `parent_id IS NULL AND
   user_id = <this user>`. Exactly one is expected to be
   `My Folder - <username>`; any extras are SQL-created siblings
   (see §10). Each row is **adopted in place** as a drive's root
   folder — no row is deleted, no descendant `parent_id` changes,
   no path-prefix strip across the whole tree.
   - The `My Folder - <username>` row → becomes the **default
     personal drive's root folder**. In one CTE statement per
     user (same shape as §3's `create_personal_drive`):
     - INSERT into `storage.drives` with `kind='personal'`,
       `default_for_user=<user>`, `quota_bytes=<user.storage_quota_bytes>`
       (no `name` column).
     - UPDATE the wrapper folder row: set `drive_id=<new drive>`
       and `name='Personal'` (renames the wrapper to the canonical
       default name; the BEFORE-UPDATE `path` trigger fires and
       cascades the new name down every descendant via the
       existing AFTER-UPDATE cascade trigger — no per-row UPDATE
       in the migration script).
     - UPDATE `storage.drives` to set `root_folder_id=<wrapper row id>`.
     - INSERT one `role_grants` row: subject=`<user>`,
       `resource_type='drive'`, `resource_id=<new drive>`,
       `role='owner'`.
   - Every other sibling row → becomes a fresh **secondary
     personal drive's root folder**, same four-write CTE shape
     except: `default_for_user=NULL`, no `name` change (the
     sibling keeps its original name), and the Owner grant points
     at the same user. Membership rules from §2 apply
     (single-owner, no `add_member`); the user can later promote
     one to `kind='shared'` to invite collaborators.
4. **Cascade `drive_id` down the tree** — for every folder/file
   row, set `drive_id` by walking the ancestry up to whichever
   adopted root the row descends from. After this step every row
   has the same `drive_id` as its `parent_id`'s row, which chains
   up to a root folder whose `drive_id` was set in step 3.
   Reuse the existing ltree-aware recursive helper
   (`storage.copy_folder_tree`-style descent) — single
   `UPDATE … WHERE` per drive, not a row-at-a-time loop.
5. **No bulk path rewrite.** The `path` column on descendants is
   untouched by this migration. The wrapper rename in step 3
   (`My Folder - admin` → `Personal`) is the only path-affecting
   change; the BEFORE-UPDATE folder trigger rewrites the wrapper
   row's own `path` / `lpath`, and the AFTER-UPDATE cascade
   trigger propagates the new path prefix to every descendant
   automatically.
   - **Downstream caches and indexes** — most are unaffected
     because path *content* changes only inside the renamed
     wrapper segment (descendants reflect "Personal/…" instead of
     "My Folder - admin/…"). Audit:
     - **Tantivy content index (§11)** — the index does NOT
       store paths (see `tantivy_content_index.rs`: indexed
       fields are `file_id`, `user_id`, `name` (basename only),
       `content`. No `path` field). Reindex IS still required —
       not because of paths but because the schema gains
       `drive_id` and the query filter pivots from `user_id` to
       `drive_id`. Same migration window, different reason.
     - **Thumbnail cache** — file_id-keyed: unaffected by the
       wrapper rename. Path-keyed entries (if any) invalidate on
       any path change in the wrapper; flush as a precaution and
       switch to file_id keying during this migration if not
       already done.
     - **Folder ETag queue (`async_tree_etag_queue`,** see Open
       Question 8) — recompute. The wrapper rename touches the
       wrapper's own ETag at minimum; ancestors-of-ancestors
       below the wrapper are structurally unchanged.
     - **Recent-items / favorites** — referenced by file_id, not
       path; unaffected.
   - **On-disk storage mirror** — see Open Question 10. If the
     filesystem layout mirrors `path`, the wrapper directory
     itself is renamed (one `mv`) and the descendant directories
     don't move; the rename is atomic on the filesystem. If
     content-addressable, the FS is untouched.
6. Verify: every row has `drive_id IS NOT NULL`; every drive has
   `root_folder_id IS NOT NULL` and pointing at a real folder row
   whose `parent_id IS NULL` and whose `drive_id` matches the
   drive's id (the 1:1 invariant from §3); no row has `parent_id`
   pointing at a non-existent folder.
7. Add `NOT NULL` constraints: `drive_id` on `storage.folders`
   and `storage.files`. `root_folder_id` on `storage.drives`
   stays NULLable at the column level (§3 explains why — the
   atomic CTE writes NULL on the drive INSERT and populates the
   column with an UPDATE later in the same statement; a column-
   level `NOT NULL` would refuse the initial INSERT). The
   invariant "every drive has a root folder" is enforced by the
   CTE being the only creation path, not by a constraint.
   Verification step 6 checks the invariant on the populated
   dataset; ongoing enforcement is application-layer.
8. Land the **no-orphan-root-folder constraint trigger** (§3,
   "DB-level invariant"): `AFTER INSERT OR UPDATE` on
   `storage.folders`, fires when `NEW.parent_id IS NULL`, refuses
   unless the matching drive has its `root_folder_id` pointing at
   the row. `DEFERRABLE INITIALLY DEFERRED` so the atomic
   four-write transaction commits cleanly. Includes a pre-flight
   `DO`-block that refuses the migration if any existing orphan
   root folder is present (catches historical bad data at migration
   time). **Direct test coverage is deferred to D3** — writing the
   positive-path test today would require reimplementing the
   atomic create flow in bash, which Ed rejected as duplication.
   D0 ships with transitive coverage via `drives_foundation.hurl`
   (the lifecycle hook exercises the trigger end-to-end through
   the production path); the dedicated test rides alongside D3's
   create-shared-drive API.

**Keep `user_id`** on resources alongside `drive_id` for the entire
Phase A release cycle. Code is updated to read `drive_id` everywhere;
`user_id` is dual-written for one release as a safety net. If
something breaks we can roll back without data loss.

Pre-flight checks the migration script runs before any writes:
- Report how many sibling-root folders exist per user. Don't
  refuse — extras become drives — but surface the count so an
  operator can sanity-check ("Ed has 4 root folders, expected
  ≤1; verify those are real and intended before promoting them
  to drives").
- (Obsolete with the `/drives/<uuid>/...` top-level URL — kept
  here for historical context.) Originally the migration was
  going to refuse any sibling root folder literally named
  `drives` because the URL surface was `/webdav/drives/<uuid>/...`.
  D1 moved the explicit-drive selector to a separate top-level
  `/drives/<uuid>/...` prefix (see §9 "Native WebDAV"), so the
  collision no longer exists and the pre-check is unnecessary.
- Refuse if any user has `storage_used_bytes > storage_quota_bytes`
  by an amount that wouldn't fit the destination drive's quota
  semantics (sanity check).

### Phase B — cleanup (PR D7, one release later)

1. Drop `user_id` from `storage.folders` and `storage.files`.
2. Drop dual-write code paths.
3. Deprecate `auth.users.storage_quota_bytes` (or drop — quotas live
   on drives now).

Phase B is the point of no return; deferring it by one release gives
us a real rollback window while the new model bakes in production.

## PR sequencing

| PR | Scope | Risk |
|---|---|---|
| **D-Prep — role_grants refactor** | `access_grants → role_grants` schema migration with role-bundle semantics. `Manage` Permission added to the enum + role bundle. Engine reads role_grants only; `access_grants` removed (after one dual-write release if compat is needed). API gains `role` parameter on grant endpoints; audit log emits one `role_grant.*` event per role assignment instead of N permission events. **No Drive concept yet.** Sets the foundation that all subsequent PRs build on. **Data shape confirmed**: empirical audit shows >99% of existing `access_grants` rows already cluster into the standard bundles (viewer/editor/owner) — the migration is mechanical for the vast majority of data; the <1% edge cases get absorbed by shipping `commenter` and `contributor` roles on day one or get an explicit per-row migration decision logged. | **Medium** — touches the load-bearing authorisation table, but the data shape removes the main migration risk |
| **D0 — foundation** | `storage.drives` schema (no `drive_members` — uses `role_grants` from D-Prep); `Drive` domain entity; migration creating personal drives + backfilling `drive_id` on every resource; read-only `GET /api/drives` listing the caller's drives (single query: `SELECT … FROM role_grants WHERE subject_id=$caller AND resource_type='drive'`). Dual-write `user_id` alongside `drive_id` for safety. **No new UI.** **Every upload path stamps `drive_id` at insert**: classic multipart (`file_handler::upload`), chunked NC (`uploads_handler`), streaming CDC (`upload_ingest`), delta upload (`delta_upload_service`), instant upload by hash. Tantivy reindex (see §11) is part of this PR. **Provenance columns added** (see §14): `created_by` and `updated_by` on both `storage.folders` and `storage.files`, FK to `auth.users` with `ON DELETE SET NULL`; backfilled from `user_id` so pre-Drive content has provenance from day one; every mutation path that touches `updated_at` also sets `updated_by`. | **High** — every storage query touches, all upload paths touched |
| **D1 — UI switcher + URL routing** | Sidebar drive picker, `/files/<folder-id>` reused for cross-drive navigation (existing route — drive context recovered server-side from `folders.drive_id`), `/config/drive/<drive-uuid>` new route for drive admin. `/` redirects to `/files/<root-folder-id>` of the caller's default personal drive (internal users) or `/shared-with-me` (external users with no personal drive). Native WebDAV gets a new `/webdav/@drive/<uuid>/...` route alongside the existing `/webdav/<path>` (which keeps mapping to the caller's default drive — zero back-compat breakage); the `@drive` sigil keeps everything WebDAV under one URL root with near-zero collision risk. NC keeps the credential-side scheme — see §9. `/drive/<...>` (singular, on the SPA) reserved for future use. | Medium |
| **D2 — drive membership API + per-drive trash auth** | `POST /api/drives/{id}/members`, `DELETE`, `PUT` for role changes — thin handlers that translate to `role_grants` INSERT/DELETE/UPDATE with `resource_type='drive'`. `Resource::Drive(Uuid)` (added in D-Prep at the enum level) gets its specialised handler surface here. Shared-drive last-owner protection. Group-as-subject support reuses the existing `subject_groups` machinery. **Personal-drive guards** (`add_member`, `remove_member`, `delete_drive` refuse on `kind='personal'` — see §2). **Per-drive trash authorisation** (§12): trash listing filters by drive(s) the caller can read; trash mutations (send/restore/permanent-delete) require `role='owner'` on the drive; `storage.trash_items` VIEW updated to surface `drive_id`; orphan/aborted-upload sweep becomes per-drive. | Medium |
| **D3 — group-owned shared drives** | "Create shared drive" flow — admin or group owner triggers, drive created with `kind='shared'`, initial owner row is the group. Group-deletion guard refuses if the group is the last owner of any drive. Drive-rename, drive-delete. | Low |
| **D4 — per-drive quota** | Move storage accounting off `auth.users.storage_used_bytes` onto `storage.drives.used_bytes`. **Re-point the existing per-user incremental CTE** (introduced in v0.7.0 — see `b5b80549`, `d6987329`) at drive rows; don't reinvent the counting logic. Upload paths check `drive.quota_bytes` instead of (or in addition to) the user's quota for the dual-write window. **Per-chunk incremental quota check on the NC chunked path** (see §13): MKCOL refuses when the drive is already over quota; each PUT chunk runs an O(1) `used + session_so_far + chunk_size > quota` test and refuses with 507 within one chunk of wasted upload. Closes a pre-existing wart where NC clients could upload GB before learning they were over quota. Reconciliation job runs once per day to fix drift. | Medium |
| **D5 — policies** | JSONB policies column + enforcement at the known callsites. **Mutation is OxiCloud-admin only** (the original "owner-mutable" plan made policies self-policing soft caps — see §8). Five policies in v1: `forbid_public_links`, `forbid_external_sharing`, `forbid_sharing`, `forbid_cross_drive_move`, and `forbid_owner_role_change`. Ship one at a time if you want fine-grained rollout in that order. | Low |
| **D6 — cross-drive move + audit** | Move folder/file between drives (allowed by default; gated by `forbid_cross_drive_move` policy on the source drive). Audit events for every drive lifecycle event (`drive.created`, `drive.member_added`, `drive.member_removed`, `drive.policy_changed`, `drive.deleted`, `resource.moved_between_drives`). | Low |
| **D7 — back-compat sweep** | Drop `user_id` from `storage.folders` / `storage.files`. Drop dual-write code. Drop or deprecate `auth.users.storage_quota_bytes`. **Provenance columns (`created_by`, `updated_by`) stay** — they were populated from D0 and are now the sole source of authorship signal. | Low — but the point of no return |

Approximate total: 4–6 weeks of focused work, depending on test
coverage depth.

## Out of scope for v1 (worth noting so we don't accidentally invite scope creep)

- **Timeboxed session policy** — drives that auto-lock after N
  minutes. Big middleware lift.
- **End-to-end encryption policy** — client-side encryption with
  server holding only ciphertext. Massive scope.
- **Per-drive sync targets** — sync client design ("1 drive = 1
  target like 'family-computer'"). Schema-friendly today (drive has
  stable UUID), implementation is its own design.
- **Cross-drive search UI** — start with per-drive scoping. "Search
  everywhere" comes later.
- **Drive templates** — "create a drive pre-populated with these
  folders" is nice but not foundation work.
<!--
Per-drive trash was originally listed as deferred until D7. It is
now in-scope and ratified in §12 — moved into D2 (trash permission
checks become drive-role-aware once the membership API lands).
The storage cost is near-zero because trash is soft-delete-in-
place on `storage.files` / `storage.folders`, which already carry
`drive_id` after Phase A.
-->


## Open questions to revisit before D0 lands

1. **Naming of the default personal drive**.
   - Locked: "Personal" (i18n key, neutral, doesn't break on username
     rename).
   - But: should it be renameable by the user? Default-rename allowed?
     I'd say yes — drive name is just a label.

2. **`auth.users.storage_quota_bytes` — drop or keep?**
   - Drop entirely (cleanest).
   - Keep as the **default initial quota** for newly-created personal
     drives (so admins still have a single tunable).
   - I lean toward keep-as-default-initial.

3. **Can a user be a member of their own personal drive in more than one
   way?** (e.g. directly AND via a group they belong to)
   - Should the membership rows allow that? Or dedup?
   - Easy answer: allow; permission resolution naturally unions, so
     duplicate paths to the same role are idempotent.

4. **What happens when you DELETE a folder/file in a shared drive
   you have only editor role on?**
   - Today: ReBAC checks Delete permission on the resource.
   - With drives: editor role implies Create + Update + Comment but
     **not** Delete (per the role-bundle table above).
   - So editor cannot delete by default. Should they? Some teams want
     "editor can do anything except change drive settings". Worth a
     dedicated decision before D2 — possibly add a separate "can
     delete" toggle, or split editor into `editor` / `contributor`.

5. **Drive icons / colour customisation** — visual differentiator
   between drives in the sidebar. Out of scope for v1 but worth
   noting; the `drives` table can carry a small `display` JSONB column
   that accumulates this kind of cosmetic config without schema
   churn.

6. **WebDAV path resolution edge case**: when a user is a member of a
   shared drive and someone shares a single file inside it explicitly
   with them via a per-resource grant, what path do they see in their
   client?
   - The drive's path (they have access via membership), full stop.
   - Per-resource grants are additive; they don't create a separate
     "shared with me" listing for files that already live in a drive
     the user can access.
   - Confirm during D2 that this is what users expect.

7. **Search-everywhere from the file picker**. When a user goes to
   share a file, today the picker lists their own files. With drives,
   should the picker default to "current drive" or "all drives I have
   access to"? UX call, not a foundation decision.

8. **`async_tree_etag_queue` — drive-pinning assumption audit**.
   v0.7.0 introduced `migrations/20260626000000_tree_etag_statement_triggers.sql`
   + `20260627000000_async_tree_etag_queue.sql` for ETag
   propagation up folder ancestry. The queue likely keys off
   folder path / owner; before D0 starts, confirm it doesn't
   embed assumptions that fold `drive_id` away (e.g. computing
   ancestry across a path that crosses a drive boundary).
   5-minute audit to lock the question down.

9. **NC credential ↔ drive binding default**. Section 9 leaves
   open whether the Login Flow v2 picker defaults to issuing
   `{user}~{uuid}` Basic Auth usernames (option 1) or
   `auth.app_passwords.drive_id`-bound app-passwords (option 2)
   for non-personal drives. Decide before D1 ships; the answer
   determines whether `auth.app_passwords` gets a new column or
   not.

10. **On-disk storage mirror — does the file path under
    `OXICLOUD_STORAGE_PATH` change too?** Phase A step 3 renames
    the wrapper folder row (`My Folder - admin` → `Personal`) for
    each default personal drive; the AFTER-UPDATE trigger
    rewrites descendant `storage.folders.path` /
    `storage.files.path` values automatically (no bulk UPDATE in
    the migration script). If the on-disk layout mirrors these
    paths (`<storage>/<user_id>/My Folder - admin/Docs/foo.pdf`),
    the migration ALSO has to rename the wrapper directory on
    disk — **but only the wrapper directory itself**, one `mv`
    per drive, atomic on the filesystem; no descendant `mv`
    needed. If on-disk is content-addressable (BLAKE3-keyed),
    the columns can be rewritten without touching the filesystem
    at all. **Audit the storage adapter before starting D0** and
    decide whether the migration script:
    - just lets the trigger rewrite the path columns (CAS layout
      — cheap), or
    - rewrites the path columns AND issues a single `mv` per
      drive on disk (path-mirrored layout — still cheap; only
      the wrapper directory moves, the subtree comes along for
      free).
    The blob store is content-addressable as of v0.7.0 so most
    file content lives under `.blobs/<hash[..2]>/<hash>` and is
    already wrapper-agnostic; the concern is only the
    metadata-projection / thumbnail-cache trees if they're
    path-shaped.

11. **Promoting sibling root folders — UX after migration**.
    Users whose home was a single `My Folder - <username>` end
    up with one drive (Personal). Users with SQL-added siblings
    end up with multiple drives, one named per the original
    folder name. The drive name carries over verbatim — should
    the migration log who got more than one drive, so an
    operator can DM them and explain the new picker? Operational
    nicety, not a correctness concern.

12. **Deleted-user tombstone for provenance**. §14 sets
    `created_by` / `updated_by` to NULL when the referenced user
    is deleted (`ON DELETE SET NULL`). The UI then shows "Unknown
    user", which is correct but information-poor — for compliance
    / audit / "who left this?" purposes, a small `deleted_users`
    tombstone table (`id, last_known_username, deleted_at`) would
    let the UI render "Bob (deleted 2026-04-12)" instead of just
    "Unknown user". Out of scope for v1 but easy to add later
    without schema rework (NULL `created_by` stays NULL; a
    parallel lookup against the tombstone table on display).

## Existing code to reuse

- **ReBAC `Resource` enum** at `src/domain/services/authorization.rs:74`
  already has a tagged-union shape; adding `Drive(Uuid)` is one variant
  + `from_parts` arm + `type_str` arm.
- **ReBAC `Permission` enum** already has the bundle we need
  (`Read`, `Create`, `Update`, `Delete`, `Share`, `Comment`). The
  role-to-bundle mapping for drives lives in a new `drive_role.rs`
  helper.
- **`PgAclEngine` Moka cache** at
  `src/infrastructure/services/pg_acl_engine.rs:88` handles ReBAC
  grant caching. The drive-role check piggybacks on the same cache
  with a separate keyspace (`drive_perm:<user>:<drive>:<perm>`).
- **`SubjectGroupService::list_transitive_users`** at
  `src/application/services/subject_group_service.rs:385` already
  expands a group to its transitive user members. The drive-membership
  check uses this to resolve group-owner subjects.
- **`folder_service::create_home_folder`** at
  `src/application/services/folder_service.rs:644` is where the
  per-user wrapper folder is created today. Post-migration the
  function is **replaced** by a single `create_personal_drive`
  call against `DriveRepository` that runs the §3 atomic CTE:
  drive + root folder (named "Personal", `parent_id=NULL`,
  `drive_id` pinned) + Owner `role_grants` row, all in one SQL
  statement. The lifecycle path is the same; the work moves to
  the drive repository.
- **NC path resolver `nc_to_internal_path`** at
  `src/interfaces/nextcloud/webdav_handler.rs:51` and the native
  resolver `resolve_webdav_path` at
  `src/interfaces/api/handlers/webdav_handler.rs:188` are the two
  callsites that learn about drives. Both gain a "drive context"
  parameter resolved from the URL prefix (`/files/<u>/` or
  `{user}~{uuid}` for NC; `/webdav/` or `/webdav/drives/<uuid>/`
  for native). Each resolves to the drive's root folder via
  `drives.root_folder_id` and prepends that folder's `path`
  (after the migration this is `Personal/…` for default personal
  drives, the original sibling-root name for secondaries, the
  shared-drive root name for shared drives). The personal-vs-shared
  branch is now a single metadata lookup, not a path-shape decision.
- **`MagicLinkInviteService`** and the share-notification pipeline
  (`RecipientNotificationService`) need the new policy checks
  (`forbid_external_sharing`, `forbid_sharing`) wired in at their
  respective callsites.
- **Migration timestamp convention**: as of v0.7.0 the head
  migration is `20260702000000_drop_dead_file_indexes.sql`, and
  `20260701000000_content_search_index.sql` already exists (the
  Tantivy index). D0's migration becomes `20260801000000_drives.sql`
  (or whatever date D0 actually starts) — the original
  `20260701000000_drives.sql` slot is taken.

## Verification

Each PR carries its own verification block. The bar across all of
them: **(a)** new behaviour proven by a focused test, **(b)** the
existing Hurl + Playwright baselines still pass green (`bash
tests/api/run.sh && bash tests/webdav/run.sh && cd tests/e2e && npm
test`), **(c)** `cargo fmt && cargo clippy --all-features
--all-targets -- -D warnings` clean.

### D-Prep
- **Unit**: `role_bundle(role)` returns the expected permission set
  for each role; `roles_implying(permission)` returns the expected
  role list; round-trip a grant insert→read→compare for every role.
- **Data audit**: query `access_grants` and confirm that >99% of
  rows cluster into the standard bundles (the empirical figure that
  unlocked Sequence A). Log the <1% edge cases per row for review.
- **Migration round-trip**: roll forward against a populated DB →
  every prior `access_grants` row is represented by exactly one
  `role_grants` row → roll back → original `access_grants` rows
  recovered byte-identically.
- **API**: new Hurl test `tests/api/role_grants.hurl` —
  - `POST /api/grants` with `role='editor'` creates a single row
    with the expected bundle when expanded.
  - `PUT /api/grants/{id}` with `role='viewer'` is atomic (no race
    window where the user has zero permissions).
  - Compat shim: `POST /api/grants` with the legacy `permission`
    field still works for one release and maps to the closest role.
  - Audit log emits `role_grant.created`, `role_grant.role_changed`,
    `role_grant.revoked` events with the role name carried through.
- **UI smoke (Playwright)**: My Shares dialog renders roles instead
  of permission checkboxes; share modal preset buttons map to roles;
  changing a member's role from editor → viewer fires exactly one
  PATCH and the UI reflects the change without a race-window blank
  state.
- **All existing Hurl + WebDAV + Playwright tests still pass** — the
  refactor must be invisible to every non-grant-handling test.

### D0
- **Unit**: `Drive` entity (kind + `default_for_user` CHECK
  constraints), `default_for_user` partial unique index. Personal-
  drive `add_member` / `remove_member` / `delete_drive` all refuse
  cleanly with the expected `DomainError`.
- **Migration round-trip**: roll forward against a populated DB →
  every existing folder/file row has `drive_id` set (no NULLs);
  every drive has `root_folder_id IS NOT NULL` and the row it
  points at has `parent_id IS NULL AND drive_id = <self>` (the
  1:1 invariant from §3); every user has exactly one drive with
  `default_for_user` set; sibling root folders became secondary
  `kind='personal'` drives whose root folders kept their original
  names; default-personal wrapper folders were renamed from
  `My Folder - <username>` to `Personal` and the AFTER-UPDATE
  trigger cascaded the rename down the descendant `path` values
  → roll back via `sqlx migrate revert` → `drive_id` /
  `root_folder_id` columns gone, `user_id` intact thanks to
  dual-write, wrapper folder names restored to
  `My Folder - <username>` (and the trigger cascade restores
  descendant paths).
- **Storage check**: post-migration `bash tests/api/storage_cleanup_check.sh`
  still reports a clean tree (no orphans).
- **Tantivy reindex**: every indexed doc carries a `drive_id`;
  search query filtered by `drive_id ∈ caller's drives` returns the
  expected hits; cross-drive isolation confirmed (search as Alice,
  Bob's content never appears).
- **API**: `tests/api/drives_foundation.hurl` — admin lists
  `/api/drives`, sees their default personal drive with the correct
  quota, `kind='personal'`, `default_for_user` matching their own
  uuid. Creating a folder still works; the folder's `drive_id`
  matches the personal drive.
- **All upload paths set `drive_id`**: targeted Hurl tests for
  multipart, native chunked, NC chunked, delta upload, instant
  upload by hash. Each verifies the resulting file row has the
  expected `drive_id`.

### D1
- **Routing**: `cargo build` clean; WebDAV dispatcher routes
  `/webdav/@drive/<uuid>/...` correctly (also accepts the
  URL-encoded `/webdav/%40drive/<uuid>/...` form);
  `/webdav/<path>` still resolves to the caller's default drive
  (back-compat).
- **Collision guard**: MKCOL / PUT / REST refuse creation of a
  folder literally named `@drive` at any drive root (case-sensitive,
  exact match — sub-folders named `@drive` deeper in the tree are
  allowed, the guard is only at root depth).
- **NC client back-compat**: a real NC sync client pointed at
  `/remote.php/dav/files/admin/` continues syncing the user's default
  personal drive without reconfiguration. The chroot POC's `~`
  username (or app-password binding) lands a sync into the chosen
  drive transparently.
- **Manual smoke (internal user)**: open `/`, get redirected to
  `/files/<default-personal-drive-root-folder-id>`. Click sidebar
  drive switcher → URL updates to `/files/<picked-drive-root-folder-id>`,
  listing reloads. Drive picker shows all of the caller's drives
  (default first), each with its quota usage. Open
  `/config/drive/<personal-drive-uuid>` → owner sees member list +
  policies. Open `/config/drive/<shared-drive-uuid>` as a viewer →
  read-only "Drive info" surface.
- **Manual smoke (external user)**: open `/`, get redirected to
  `/shared-with-me` (no `/files/<id>` for an account without a
  personal drive).
- **Playwright**: a new `tests/e2e/drive-switching.spec.ts` exercises
  sidebar → URL → listing → cross-drive isolation (folders in
  drive A don't appear in drive B's listing), plus the
  internal-vs-external root redirect split.

### D2
- **Membership API**: `POST /api/drives/{id}/members` with user, with
  group, and with role changes; refuses on personal drives;
  shared-drive last-owner protection. Group expansion (transitive
  members count toward the owner total).
- **Trash per drive**: trash listing scopes correctly to drive(s)
  the caller can read; mutations refuse without owner role; the
  `storage.trash_items` VIEW surfaces `drive_id`.
- **Updated NC + WebDAV regression baselines** — the 105+ scenarios
  in `BASELINE_TESTS_NC_WEBDAV.md` still pass green; per-drive
  trash and membership changes don't regress existing protocol
  behaviour.

### D3–D7
- Per-PR verification authored when the PR is drafted. Each PR adds
  at least one new Hurl/Playwright test for the headline capability
  and proves zero regression on the previous baselines.

### Cross-cutting regression — runs on every Drive PR

A standing checklist independent of the headline capability of each
PR:

1. `bash tests/api/run.sh` green (>100 scenarios).
2. `bash tests/webdav/run.sh` green (NC + native WebDAV baselines).
3. `cd tests/e2e && npm test` green (Playwright).
4. `tests/api/storage_cleanup_check.sh` clean.
5. No new `cargo clippy` warnings.
6. Tantivy index returns no cross-drive results for any caller.
7. `/api/admin/dedup/stats` shows blob ref-counts consistent with
   the number of files referencing each blob across all drives.

## UI design — outline for D1 and D3

The Drive concept reshapes three load-bearing UI elements. Each
gets a small design pass alongside the relevant PR — the bullets
here lock the **intended shape**; the pixel work happens when D1 /
D3 draft.

### Sidebar — drives as children of a "Drive" section

Today the sidebar shows a single root view (the user's home
folder). Post-Drive it gains a top-level **Drive** section whose
children are the drives the caller has access to:

```
📁 Drive
  ⭐ Personal                ← kind='personal', default_for_user=caller  (★ marks default)
     Family Archive          ← kind='personal', secondary (no star)
     Engineering             ← kind='shared',  caller is owner/editor/viewer
     2025 Marketing Sprint   ← kind='shared'
  Recent
  Favorites
  Shared with me
  Trash
```

- The user's **default personal drive** is marked with a star (or
  bolded — UI choice). Clicking it lands on the default landing
  view (today's "Files" experience).
- Other drives — secondary personals, shared drives — appear below
  with the same visual weight. Clicking switches the context (URL
  changes to `/drive/<uuid>/...`, listing reloads).
- The sidebar is collapsible per-drive; users with many drives can
  hide the drives they aren't actively in.
- **No drive icons / colours in v1** (deferred to a future polish
  PR — see "Out of scope" item 5).
- The "Drive" section header itself is non-interactive (just a
  grouping label); the action affordance is "Create shared drive"
  via the `+` button at the end of the list (admin / group-owner
  scoped; lands in D3).

### Breadcrumb — drive-rooted

Today's breadcrumb is path-rooted: `Home / Docs / Q3 / Report.pdf`.
Post-Drive the breadcrumb starts at the selected drive:

```
[Personal ▾] / Docs / Q3 / Report.pdf
```

- The drive name is the **first** element. Clicking it returns to
  that drive's root listing.
- The `▾` chevron next to the drive name opens a quick-switcher
  picker (same list as the sidebar). Lets users jump between drives
  without using the sidebar.
- For paths inside shared drives, the drive name still leads:
  `[Engineering ▾] / Specs / Q3 OKRs.md`.
- The breadcrumb's overflow / truncation behaviour for deep paths is
  unchanged from today — only the root element is new.
- **Drive name follows server-side rename** — the breadcrumb queries
  the drive's current `name`; renaming the drive updates the
  breadcrumb on next reload without any client-side cache work.

### "Owners" section — review and redesign

Today's UI has an "Owner" surface in several places (file details
sidebar, share dialog, folder properties). It currently shows "the
user who owns this file" — a single name.

Post-Drive, ownership has multiple shapes:

- For a file in a personal drive: still one owner (the drive's sole
  user-owner). Display stays the same — "Owner: Ed".
- For a file in a shared drive: ownership is the drive's **owner-
  role members** (possibly multiple users + groups). Display
  becomes "Owner: Engineering team (3 members + 2 groups)".
- For a drive itself (when displayed in a settings view): same as
  above, but in a list form: "Owners: Ed, Alice, Engineering
  group".

Open questions for the D3 PR to settle:
- **Show the drive name as the effective owner?** "Owner:
  Engineering" reads cleanly but conflates "the drive" with "the
  drive's owners" — clearer for end users, less precise.
- **Should the file details sidebar expand owners on click?**
  Click "Engineering team" → see the owner roster. Useful for
  large drives where the owner list doesn't fit inline.
- **Audit/My Shares dialog**: the "Shared with me" view currently
  groups by sharer. Post-Drive it can group by drive instead
  (your shared-drive access shows once per drive, not once per
  file). UX win — decide which grouping is default and whether
  both are togglable.

These three UI surfaces are independent enough that they can ship
in separate PRs (sidebar in D1, breadcrumb in D1, owners review
in D3 alongside the create-shared-drive flow). The sidebar and
breadcrumb are essentially mechanical given the new model; the
owners review is the only one with genuine design questions.

## File map (anticipated, not yet created)

```
migrations/
  20260730000000_role_grants.sql            ← D-Prep (rename of access_grants
                                              to role_grants with role bundles)
  20260801000000_drives.sql                 ← D0 (drives table only;
                                              membership is rows in role_grants)
  20260901000000_drop_user_id_on_resources.sql ← D7

src/domain/entities/
  drive.rs                                  ← D0
  role.rs                                   ← D-Prep (role enum + bundle map)

src/domain/services/
  authorization.rs (modify)                 ← D-Prep adds `Manage` Permission +
                                              `Resource::Drive(Uuid)` variant

src/application/services/
  drive_service.rs                          ← D0–D3 (CRUD + policy enforcement,
                                              membership operations translate
                                              to role_grants writes)

src/infrastructure/repositories/pg/
  drive_pg_repository.rs                    ← D0
  role_grant_pg_repository.rs               ← D-Prep (replaces
                                              access_grant_pg_repository)

src/infrastructure/services/pg_acl_engine.rs (modify)
  reads role_grants only                    ← D-Prep
  Resource::Drive routing                   ← D0

src/interfaces/api/handlers/
  grant_handler.rs (modify)                 ← D-Prep (accepts role parameter)
  drive_handler.rs                          ← D0 (list), D2 (members), D3 (create/delete shared)

src/interfaces/api/handlers/webdav_handler.rs (modify)
  drive-aware path resolution               ← D1

src/interfaces/nextcloud/webdav_handler.rs (modify)
  drive-aware path resolution               ← D1

static/js/views/drive/                      ← D1 (sidebar switcher, drive view)
static/js/model/drives.js                   ← D1 (REST client)
static/css/components/driveSwitcher.css     ← D1
```

## Glossary (for the next reader)

- **Drive** — a top-level container that owns folders and files.
- **Personal drive** — the auto-created drive a user gets at
  registration. One per internal user. `kind='personal'`.
- **Shared drive** — a drive whose membership includes at least one
  group (or multiple users). `kind='shared'`.
- **Drive member** — a row in `storage.role_grants` with
  `resource_type='drive'`, carrying a subject (user or group)
  and a role. There is **no** separate `drive_members` table — see
  the D-Prep prerequisite at the top of this plan for the storage
  pivot.
- **Owner / editor / viewer** — drive roles. Map to ReBAC
  permission bundles for resources inside the drive.
- **Drive policy** — JSONB key on the drive that toggles a sharing /
  movement restriction.
- **Drive context** in WebDAV — the drive whose root the request is
  rooted at. Native WebDAV resolves it from the URL prefix
  (`/webdav/<path>` → caller's personal drive,
  `/webdav/drives/<uuid>/<path>` → explicit drive). NC resolves
  it from the credential (Basic Auth `{user}~{uuid}` username, or
  the `auth.app_passwords.drive_id` binding — see §9).
- **Wrapper folder** — historical name for
  `My Folder - <username>`, the folder created at registration
  via `format!("My Folder - {}", username)`. **Adopted** in the
  Drive migration: the same folder row is renamed to `Personal`
  and reused as the default personal drive's root folder
  (`drives.root_folder_id`). No row is deleted; the wrapper IS
  the root folder under the new model. Every reference to
  "wrapper" in older comments / docs is by definition pre-Drive.
- **Drive's root folder** — the folder row pointed at by
  `storage.drives.root_folder_id`. `parent_id IS NULL`,
  `drive_id` = the drive. Every drive has exactly one (§3); the
  drive's display name lives on this folder's `name` column.
