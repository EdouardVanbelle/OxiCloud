//! `load-seed` — bulk-seeds OxiCloud's test database for K6 load scenarios.
//!
//! Inserts users, a deep folder tree, files (all sharing one dedup'd blob),
//! nested subject groups, and ReBAC grants directly via sqlx — bypassing the
//! REST API so the seed phase is fast and doesn't pollute measured metrics.
//!
//! Only the resources each k6 scenario actively touches (the grant being
//! created, the move target, etc.) go through the HTTP API at run time.
//!
//! Run (from repo root, against the test DB on port 5433):
//!
//! ```bash
//! DATABASE_URL='postgres://oxicloud_test:oxicloud_test@localhost:5433/oxicloud_test' \
//!   cargo run --bin load-seed -- \
//!     --depth 8 --fanout 3 --files-per-leaf 3 \
//!     --extra-users 20 --group-depth 3 --group-fanout 5 \
//!     --manifest tests/load/results/seed-manifest.json
//! ```
//!
//! Output: writes a JSON manifest at `--manifest` listing the IDs and
//! credentials each k6 scenario needs (admin login, grantee user, deep folder
//! IDs, group chain, etc.). K6 scenarios load this manifest at startup.

use argon2::password_hash::SaltString;
use argon2::{Algorithm, Argon2, Params, PasswordHasher, Version};
use rand_core::OsRng;
use serde::Serialize;
use sqlx::PgPool;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::PathBuf;
use uuid::Uuid;

// ── CLI ──────────────────────────────────────────────────────────────────

struct Args {
    depth: u32,
    fanout: u32,
    files_per_leaf: u32,
    extra_users: u32,
    group_depth: u32,
    group_fanout: u32,
    manifest: PathBuf,
    password: String,
}

impl Args {
    fn parse() -> Self {
        // Defaults match tests/load/test.env; both bound the exponential cost
        // of fanout^depth. Override via flags or LOAD_* env vars.
        let mut depth = 5;
        let mut fanout = 4;
        let mut files_per_leaf = 3;
        let mut extra_users = 20;
        let mut group_depth = 3;
        let mut group_fanout = 5;
        let mut manifest = PathBuf::from("tests/load/results/seed-manifest.json");
        let mut password = "TestPassword1!".to_string();

        let argv: Vec<String> = env::args().collect();
        let mut i = 1;
        while i < argv.len() {
            let key = argv[i].as_str();
            let val = || {
                argv.get(i + 1)
                    .unwrap_or_else(|| panic!("missing value for {}", key))
            };
            match key {
                "--depth" => depth = val().parse().expect("--depth must be u32"),
                "--fanout" => fanout = val().parse().expect("--fanout must be u32"),
                "--files-per-leaf" => {
                    files_per_leaf = val().parse().expect("--files-per-leaf must be u32");
                }
                "--extra-users" => {
                    extra_users = val().parse().expect("--extra-users must be u32");
                }
                "--group-depth" => {
                    group_depth = val().parse().expect("--group-depth must be u32");
                }
                "--group-fanout" => {
                    group_fanout = val().parse().expect("--group-fanout must be u32");
                }
                "--manifest" => manifest = PathBuf::from(val()),
                "--password" => password = val().clone(),
                "--help" | "-h" => {
                    print_help();
                    std::process::exit(0);
                }
                other => panic!("unknown argument: {}", other),
            }
            i += 2;
        }

        Self {
            depth,
            fanout,
            files_per_leaf,
            extra_users,
            group_depth,
            group_fanout,
            manifest,
            password,
        }
    }
}

fn print_help() {
    println!("load-seed — bulk-seed OxiCloud for k6 load tests");
    println!();
    println!("Reads DATABASE_URL from env. Inserts users, folders, files, groups, grants.");
    println!("Writes a JSON manifest the k6 scenarios consume.");
    println!();
    println!("Flags (with defaults):");
    println!("  --depth 5             folder tree depth (exponential: fanout^depth folders)");
    println!("  --fanout 4            children per non-leaf folder");
    println!("  --files-per-leaf 3    files inserted in each leaf folder");
    println!("  --extra-users 20      non-admin users created");
    println!("  --group-depth 3       nested subject-group chain length");
    println!("  --group-fanout 5      users added directly to each leaf group");
    println!("  --manifest <path>     output manifest JSON path");
    println!("  --password <pw>       shared password for all seeded users");
}

// ── Manifest (consumed by k6 scenarios) ──────────────────────────────────

#[derive(Serialize)]
struct Manifest {
    admin: UserCreds,
    grantee: UserCreds,
    group_member: UserCreds,
    shared_subtree: SubtreeIds,
    group_subtree: SubtreeIds,
    nested_groups: NestedGroupIds,
    /// Total folders seeded (informational).
    total_folders: u64,
    /// Total files seeded (informational).
    total_files: u64,
}

#[derive(Serialize)]
struct UserCreds {
    id: Uuid,
    username: String,
    password: String,
}

#[derive(Serialize)]
struct SubtreeIds {
    /// Root of the subtree the grant is attached to.
    root: Uuid,
    /// A folder at depth 4 inside the subtree (for mid-depth fetches).
    depth4: Uuid,
    /// A folder at depth 8 inside the subtree (for mid-depth fetches).
    depth8: Uuid,
    /// A folder at depth N (== `--depth`) inside the subtree (for deep fetches).
    deepest: Uuid,
}

#[derive(Serialize)]
struct NestedGroupIds {
    /// Outermost group — the one referenced by the grant. Contains `mid` as a member.
    root: Uuid,
    /// Intermediate group — member of `root`, contains `leaf` as a member.
    mid: Uuid,
    /// Innermost group — member of `mid`, contains `group_member` user as a direct member.
    leaf: Uuid,
}

// ── Main ─────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Best-effort: load .env if present (mirrors the rest of the codebase).
    let _ = dotenvy::dotenv();

    let args = Args::parse();
    let database_url = env::var("DATABASE_URL").map_err(|_| {
        "DATABASE_URL must be set (e.g. postgres://oxicloud_test:oxicloud_test@localhost:5433/oxicloud_test)"
    })?;

    println!(
        "[load-seed] connecting to {}",
        sanitize_url_for_log(&database_url)
    );
    let pool = PgPool::connect(&database_url).await?;

    println!("[load-seed] wiping previous load-test data…");
    wipe(&pool).await?;

    println!("[load-seed] hashing shared password…");
    let password_hash = hash_password(&args.password);

    println!(
        "[load-seed] inserting users (1 admin + {})…",
        args.extra_users
    );
    let admin = insert_user(&pool, "load_admin", "admin", &password_hash).await?;
    let extra_users = insert_extra_users(&pool, args.extra_users, &password_hash).await?;

    // Pick a grantee (user[0]) and a group member (user[1]).
    let grantee = extra_users
        .first()
        .ok_or("need at least one extra user for grantee")?
        .clone();
    let group_member = extra_users
        .get(1)
        .ok_or("need at least two extra users")?
        .clone();

    println!("[load-seed] inserting shared blob…");
    let blob_hash = insert_shared_blob(&pool).await?;

    println!(
        "[load-seed] building folder tree (depth={}, fanout={})…",
        args.depth, args.fanout
    );
    // Two parallel subtrees off root: one for the user-grant scenario, one for
    // the group-grant scenario. Each has its own depth/fanout shape so a single
    // grant cascades over a known fixed number of descendants.
    // Post-D0: every folder needs a drive_id (NOT NULL) and root folders
    // (parent_id IS NULL) are reserved for the atomic drive-creation
    // transaction. The load-seed subtrees nest under admin's default
    // personal drive's root folder ("Personal") — they're a workload
    // shape, not their own drives. Look it up once and pass to both
    // build_subtree calls so they share the same parent.
    let admin_root: (Uuid,) = sqlx::query_as(
        "SELECT f.id FROM storage.folders f
           JOIN storage.drives d ON d.root_folder_id = f.id
          WHERE d.default_for_user = $1
          LIMIT 1",
    )
    .bind(admin.id)
    .fetch_one(&pool)
    .await?;
    let admin_root_id = admin_root.0;

    let shared_subtree = build_subtree(
        &pool,
        admin.id,
        admin_root_id,
        "shared_root",
        args.depth,
        args.fanout,
    )
    .await?;
    let group_subtree = build_subtree(
        &pool,
        admin.id,
        admin_root_id,
        "group_root",
        args.depth,
        args.fanout,
    )
    .await?;

    let total_folders = shared_subtree.all_ids.len() as u64 + group_subtree.all_ids.len() as u64;
    let total_leaves = shared_subtree.leaves.len() + group_subtree.leaves.len();

    println!(
        "[load-seed] inserting files (files_per_leaf={}, leaves={})…",
        args.files_per_leaf, total_leaves
    );
    let total_files = insert_files(
        &pool,
        admin.id,
        &blob_hash,
        &shared_subtree.leaves,
        &group_subtree.leaves,
        args.files_per_leaf,
    )
    .await?;

    println!(
        "[load-seed] building nested group chain (depth={}, fanout={})…",
        args.group_depth, args.group_fanout
    );
    let nested_groups = build_group_chain(
        &pool,
        admin.id,
        &extra_users,
        &group_member.id,
        args.group_depth,
        args.group_fanout,
    )
    .await?;

    println!("[load-seed] inserting grants…");
    // Grant the grantee the viewer role on shared_subtree.root — the read
    // bundle the share_cascade_rebac scenario exercises.
    insert_grant(
        &pool,
        "user",
        grantee.id,
        "folder",
        shared_subtree.root,
        "viewer",
        admin.id,
    )
    .await?;
    // Grant the outermost group the viewer role on group_subtree.root.
    insert_grant(
        &pool,
        "group",
        nested_groups.root,
        "folder",
        group_subtree.root,
        "viewer",
        admin.id,
    )
    .await?;

    let manifest = Manifest {
        admin: UserCreds {
            id: admin.id,
            username: admin.username,
            password: args.password.clone(),
        },
        grantee: UserCreds {
            id: grantee.id,
            username: grantee.username,
            password: args.password.clone(),
        },
        group_member: UserCreds {
            id: group_member.id,
            username: group_member.username,
            password: args.password.clone(),
        },
        shared_subtree: SubtreeIds {
            root: shared_subtree.root,
            depth4: shared_subtree.depth4,
            depth8: shared_subtree.depth8,
            deepest: shared_subtree.deepest,
        },
        group_subtree: SubtreeIds {
            root: group_subtree.root,
            depth4: group_subtree.depth4,
            depth8: group_subtree.depth8,
            deepest: group_subtree.deepest,
        },
        nested_groups,
        total_folders,
        total_files,
    };

    if let Some(parent) = args.manifest.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(&manifest)?;
    fs::write(&args.manifest, json)?;
    println!(
        "[load-seed] manifest written → {} ({} folders, {} files)",
        args.manifest.display(),
        total_folders,
        total_files
    );

    Ok(())
}

// ── Wipe ─────────────────────────────────────────────────────────────────

async fn wipe(pool: &PgPool) -> Result<(), sqlx::Error> {
    // Users starting with `load_` are the only ones this seeder ever creates.
    // CASCADE on auth.users → storage.folders / storage.files / grants / group
    // memberships, so a single DELETE wipes everything load-test-related.
    sqlx::query("DELETE FROM auth.users WHERE username LIKE 'load_%'")
        .execute(pool)
        .await?;
    // Groups starting with `load_` aren't owned by users; clear them explicitly.
    sqlx::query("DELETE FROM auth.subject_groups WHERE name LIKE 'load_%'")
        .execute(pool)
        .await?;
    // The shared dedup blob is identified by its fixed hash sentinel below.
    sqlx::query("DELETE FROM storage.blobs WHERE hash = $1")
        .bind(SHARED_BLOB_HASH)
        .execute(pool)
        .await?;
    Ok(())
}

// ── Password ─────────────────────────────────────────────────────────────

/// Light Argon2id parameters — same shape as the password_hasher test path
/// (`src/infrastructure/services/password_hasher.rs::test_hasher`).
/// Production hashes verify regardless of m/t/p because Argon2 reads the
/// parameters back from the hash string itself.
fn hash_password(password: &str) -> String {
    let params = Params::new(16384, 1, 1, None).expect("valid argon2 params");
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let salt = SaltString::generate(&mut OsRng);
    argon2
        .hash_password(password.as_bytes(), &salt)
        .expect("argon2 hash")
        .to_string()
}

// ── Users ────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct SeededUser {
    id: Uuid,
    username: String,
}

async fn insert_user(
    pool: &PgPool,
    username: &str,
    role: &str,
    password_hash: &str,
) -> Result<SeededUser, sqlx::Error> {
    let email = format!("{}@load.test.invalid", username);
    let row: (Uuid,) = sqlx::query_as(
        "INSERT INTO auth.users (username, email, password_hash, role)
         VALUES ($1, $2, $3, $4::auth.userrole)
         RETURNING id",
    )
    .bind(username)
    .bind(email)
    .bind(password_hash)
    .bind(role)
    .fetch_one(pool)
    .await?;
    Ok(SeededUser {
        id: row.0,
        username: username.to_string(),
    })
}

async fn insert_extra_users(
    pool: &PgPool,
    count: u32,
    password_hash: &str,
) -> Result<Vec<SeededUser>, sqlx::Error> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let usernames: Vec<String> = (0..count).map(|i| format!("load_user_{:04}", i)).collect();
    let emails: Vec<String> = usernames
        .iter()
        .map(|u| format!("{}@load.test.invalid", u))
        .collect();

    // Bulk insert via UNNEST — one round-trip for all N users.
    let rows: Vec<(Uuid, String)> = sqlx::query_as(
        "INSERT INTO auth.users (username, email, password_hash, role)
         SELECT u.username, u.email, $1::text, 'user'::auth.userrole
           FROM UNNEST($2::text[], $3::text[]) AS u(username, email)
         RETURNING id, username",
    )
    .bind(password_hash)
    .bind(&usernames)
    .bind(&emails)
    .fetch_all(pool)
    .await?;

    // RETURNING order is implementation-defined — sort by username so the
    // caller can rely on `extra_users[0]` being `load_user_0000`.
    let mut users: Vec<SeededUser> = rows
        .into_iter()
        .map(|(id, username)| SeededUser { id, username })
        .collect();
    users.sort_by(|a, b| a.username.cmp(&b.username));
    Ok(users)
}

// ── Blob ─────────────────────────────────────────────────────────────────

/// All seeded files share this single zero-byte blob. Its ref_count is bumped
/// in lockstep with file inserts; the file_metadata schema doesn't require a
/// real on-disk blob for the dedup index to be consistent.
const SHARED_BLOB_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

async fn insert_shared_blob(pool: &PgPool) -> Result<String, sqlx::Error> {
    sqlx::query(
        "INSERT INTO storage.blobs (hash, size, ref_count, content_type)
         VALUES ($1, 0, 0, 'application/octet-stream')
         ON CONFLICT (hash) DO NOTHING",
    )
    .bind(SHARED_BLOB_HASH)
    .execute(pool)
    .await?;
    Ok(SHARED_BLOB_HASH.to_string())
}

// ── Folder tree ──────────────────────────────────────────────────────────

struct Subtree {
    root: Uuid,
    depth4: Uuid,
    depth8: Uuid,
    deepest: Uuid,
    all_ids: Vec<Uuid>,
    leaves: Vec<Uuid>,
}

/// Build a folder tree under `parent_id = mount_under` rooted at `root_name`.
///
/// `mount_under` is an existing folder UUID (post-D0: the user's default
/// drive's root folder, since root folders — `parent_id IS NULL` — are
/// reserved for the atomic drive-creation transaction and would trip
/// the no-orphan-root-folder constraint trigger). Every level's
/// `drive_id` is derived from the parent folder so the NOT NULL column
/// is satisfied automatically.
///
/// Inserts level-by-level so `trg_folders_path` resolves `path`/`lpath` from
/// the already-committed parent rows. Returns the root, a depth-4 sample, the
/// deepest leaf, the full id set, and the leaf ids (used for file seeding).
async fn build_subtree(
    pool: &PgPool,
    user_id: Uuid,
    mount_under: Uuid,
    root_name: &str,
    depth: u32,
    fanout: u32,
) -> Result<Subtree, sqlx::Error> {
    // Per-row BEFORE INSERT trigger (compute_folder_path) makes each level cost
    // O(parents × fanout); past depth 6 with fanout 5 the seed time grows fast.
    // Print the count BEFORE each level so it's clear which level is in flight
    // and how many rows are about to be inserted.
    let predicted: u64 = (0..=depth).map(|l| (fanout as u64).pow(l)).sum();
    println!(
        "  subtree '{}': ~{} folders predicted",
        root_name, predicted
    );

    // Level 0 — the subtree's "root" sits inside `mount_under`, not at
    // parent_id=NULL. drive_id is inherited from the mount point.
    // Post-D7: `user_id` omitted; `created_by` / `updated_by` bind to
    // the seed caller.
    let root: (Uuid,) = sqlx::query_as(
        "INSERT INTO storage.folders
            (name, parent_id, drive_id, created_by, updated_by)
         SELECT $1, parent.id, parent.drive_id, $2, $2
           FROM storage.folders parent
          WHERE parent.id = $3::uuid
         RETURNING id",
    )
    .bind(root_name)
    .bind(user_id)
    .bind(mount_under)
    .fetch_one(pool)
    .await?;
    let root_id = root.0;
    let mut all_ids = vec![root_id];

    let mut current_level: Vec<Uuid> = vec![root_id];
    let mut depth4: Option<Uuid> = None;
    let mut depth8: Option<Uuid> = None;
    let mut deepest: Uuid = root_id;

    for level in 1..=depth {
        // Build the (parent_id, name) pairs for this level.
        let parents: Vec<Uuid> = current_level
            .iter()
            .flat_map(|p| std::iter::repeat_n(*p, fanout as usize))
            .collect();
        let names: Vec<String> = current_level
            .iter()
            .enumerate()
            .flat_map(|(pi, _)| (0..fanout).map(move |c| format!("l{}_{}_{}", level, pi, c)))
            .collect();

        let started = std::time::Instant::now();
        println!(
            "    level {}/{}: inserting {} folders…",
            level,
            depth,
            parents.len()
        );

        // drive_id derives from the parent folder — same pattern as
        // file_blob_write_repository's resolve_parent_drive helper.
        // Post-D7: `user_id` omitted; provenance via `created_by` /
        // `updated_by`.
        let rows: Vec<(Uuid,)> = sqlx::query_as(
            "INSERT INTO storage.folders
                (name, parent_id, drive_id, created_by, updated_by)
             SELECT f.name, f.parent_id, parent.drive_id, $1, $1
               FROM UNNEST($2::uuid[], $3::text[]) AS f(parent_id, name)
               JOIN storage.folders parent ON parent.id = f.parent_id
             RETURNING id",
        )
        .bind(user_id)
        .bind(&parents)
        .bind(&names)
        .fetch_all(pool)
        .await?;
        let level_ids: Vec<Uuid> = rows.into_iter().map(|(id,)| id).collect();
        println!(
            "    level {}/{}: done in {:.1}s",
            level,
            depth,
            started.elapsed().as_secs_f64()
        );

        if level == 4 {
            depth4 = level_ids.first().copied();
        }
        if level == 8 {
            depth8 = level_ids.first().copied();
        }
        if level == depth {
            deepest = *level_ids.first().expect("non-empty deepest level");
        }

        all_ids.extend(&level_ids);
        current_level = level_ids;
    }

    let depth4 = depth4.unwrap_or(deepest);
    let depth8 = depth8.unwrap_or(deepest);
    let leaves = current_level;

    Ok(Subtree {
        root: root_id,
        depth4,
        depth8,
        deepest,
        all_ids,
        leaves,
    })
}

// ── Files ────────────────────────────────────────────────────────────────

async fn insert_files(
    pool: &PgPool,
    user_id: Uuid,
    blob_hash: &str,
    shared_leaves: &[Uuid],
    group_leaves: &[Uuid],
    files_per_leaf: u32,
) -> Result<u64, sqlx::Error> {
    if files_per_leaf == 0 {
        return Ok(0);
    }
    let all_leaves: Vec<Uuid> = shared_leaves
        .iter()
        .chain(group_leaves.iter())
        .copied()
        .collect();
    if all_leaves.is_empty() {
        return Ok(0);
    }

    // Build (folder_id, name) pairs.
    let folder_ids: Vec<Uuid> = all_leaves
        .iter()
        .flat_map(|f| std::iter::repeat_n(*f, files_per_leaf as usize))
        .collect();
    let names: Vec<String> = (0..all_leaves.len())
        .flat_map(|li| (0..files_per_leaf).map(move |fi| format!("file_{}_{}.txt", li, fi)))
        .collect();

    // Post-D0: storage.files.drive_id is NOT NULL — derive it from the
    // parent folder (same pattern as file_blob_write_repository's
    // INSERTs and the resolve_parent_drive helper). The folder's
    // drive_id was set during the M2 backfill or by the lifecycle hook
    // for users provisioned after D0.
    //
    // Post-D7: `user_id` omitted; `created_by` / `updated_by` bind to
    // the seed caller so provenance is preserved.
    sqlx::query(
        "INSERT INTO storage.files
            (name, folder_id, drive_id, blob_hash, size, mime_type,
             created_by, updated_by)
         SELECT f.name, f.folder_id, fo.drive_id, $2, 0, 'text/plain', $1, $1
           FROM UNNEST($3::uuid[], $4::text[]) AS f(folder_id, name)
           JOIN storage.folders fo ON fo.id = f.folder_id",
    )
    .bind(user_id)
    .bind(blob_hash)
    .bind(&folder_ids)
    .bind(&names)
    .execute(pool)
    .await?;

    let total = folder_ids.len() as i64;
    sqlx::query("UPDATE storage.blobs SET ref_count = ref_count + $1 WHERE hash = $2")
        .bind(total)
        .bind(blob_hash)
        .execute(pool)
        .await?;

    Ok(total as u64)
}

// ── Subject groups ───────────────────────────────────────────────────────

async fn build_group_chain(
    pool: &PgPool,
    admin_id: Uuid,
    extra_users: &[SeededUser],
    group_member_user_id: &Uuid,
    depth: u32,
    fanout: u32,
) -> Result<NestedGroupIds, Box<dyn std::error::Error>> {
    if depth < 2 {
        return Err(format!("--group-depth must be ≥ 2 (got {})", depth).into());
    }

    // Create `depth` groups: g_0 (outermost) → g_1 → … → g_{depth-1} (innermost).
    // Insert one row per group; CITEXT name must match the RFC-5321 regex,
    // and the `load_` prefix lets the wipe step find them again.
    let salt: String = Uuid::new_v4().simple().to_string();
    let names: Vec<String> = (0..depth)
        .map(|i| format!("load_g_{}_{}", salt, i))
        .collect();

    let mut group_ids: HashMap<u32, Uuid> = HashMap::with_capacity(depth as usize);
    for (i, name) in names.iter().enumerate() {
        let row: (Uuid,) =
            sqlx::query_as("INSERT INTO auth.subject_groups (name) VALUES ($1) RETURNING id")
                .bind(name)
                .fetch_one(pool)
                .await?;
        group_ids.insert(i as u32, row.0);
    }

    // Wire the chain: g_i contains g_{i+1} as a member (XOR: member_group_id set).
    for i in 0..(depth - 1) {
        let parent = group_ids[&i];
        let child = group_ids[&(i + 1)];
        sqlx::query(
            "INSERT INTO auth.subject_group_members (group_id, member_user_id, member_group_id, added_by)
             VALUES ($1, NULL, $2, $3)",
        )
        .bind(parent)
        .bind(child)
        .bind(admin_id)
        .execute(pool)
        .await?;
    }

    // Innermost group gets `fanout` direct user members, ensuring the
    // group_member sentinel user is always among them.
    let leaf = group_ids[&(depth - 1)];
    let user_ids: Vec<Uuid> = {
        let mut v = vec![*group_member_user_id];
        for u in extra_users.iter().filter(|u| u.id != *group_member_user_id) {
            if v.len() as u32 >= fanout {
                break;
            }
            v.push(u.id);
        }
        v
    };

    let leafs: Vec<Uuid> = std::iter::repeat_n(leaf, user_ids.len()).collect();
    let added_by_col: Vec<Uuid> = std::iter::repeat_n(admin_id, user_ids.len()).collect();
    sqlx::query(
        "INSERT INTO auth.subject_group_members (group_id, member_user_id, member_group_id, added_by)
         SELECT g.group_id, g.member_user_id, NULL, g.added_by
           FROM UNNEST($1::uuid[], $2::uuid[], $3::uuid[]) AS g(group_id, member_user_id, added_by)",
    )
    .bind(&leafs)
    .bind(&user_ids)
    .bind(&added_by_col)
    .execute(pool)
    .await?;

    Ok(NestedGroupIds {
        root: group_ids[&0],
        mid: group_ids[&(depth / 2)],
        leaf,
    })
}

// ── Grants ───────────────────────────────────────────────────────────────

async fn insert_grant(
    pool: &PgPool,
    subject_type: &str,
    subject_id: Uuid,
    resource_type: &str,
    resource_id: Uuid,
    role: &str,
    granted_by: Uuid,
) -> Result<(), sqlx::Error> {
    // D-Prep replaced `storage.access_grants` (one row per Permission) with
    // `storage.role_grants` (one row per role assignment; the role expands to
    // a permission bundle in-code at engine read time). The seeder now writes
    // role names ('viewer'/'editor'/etc.) instead of individual permissions.
    //
    // The `role` column was promoted from TEXT to the `storage.grant_role`
    // enum by migration 20260801000000_role_grants_enum, so the cast on $5
    // is required — sqlx binds Rust &str as TEXT, which postgres won't
    // implicitly coerce into the enum.
    sqlx::query(
        "INSERT INTO storage.role_grants
            (subject_type, subject_id, resource_type, resource_id, role, granted_by)
         VALUES ($1, $2, $3, $4, $5::storage.grant_role, $6)
         ON CONFLICT (subject_type, subject_id, resource_type, resource_id) DO NOTHING",
    )
    .bind(subject_type)
    .bind(subject_id)
    .bind(resource_type)
    .bind(resource_id)
    .bind(role)
    .bind(granted_by)
    .execute(pool)
    .await?;
    Ok(())
}

// ── Utilities ────────────────────────────────────────────────────────────

fn sanitize_url_for_log(url: &str) -> String {
    // Strip "user:password@" from the URL so logs don't leak credentials.
    if let Some(at) = url.find('@')
        && let Some(scheme_end) = url.find("://")
    {
        let scheme = &url[..scheme_end + 3];
        let rest = &url[at..];
        return format!("{}***{}", scheme, rest);
    }
    url.to_string()
}
