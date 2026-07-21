-- Switch personal-drive quota semantics from "every drive owns its quota"
-- to "user envelope on the SUM of personal-drive `used_bytes`".
-- See docs/plan/drive.md §7.
--
-- Two idempotent steps:
--   1. NULL `drives.quota_bytes` for every `kind='personal'` row. After this
--      migration the column is meaningful only for shared drives; personal
--      drives' cap is `auth.users.storage_quota_bytes`.
--   2. Resync `auth.users.storage_used_bytes` to the sum-of-personal-drives
--      formula. Prior deltas may have over-counted by including shared-drive
--      uploads in the user counter; this snaps every user back to the new
--      envelope. Same shape the periodic sweep uses going forward.
--
-- Both statements `IS DISTINCT FROM`-guarded so reruns are cheap no-ops on
-- already-migrated databases. The order — NULL first, then resync — doesn't
-- matter for correctness but follows the doc's narrative.

-- 1. Drop per-drive quotas for personal drives (no-op for already-NULL rows).
UPDATE storage.drives
   SET quota_bytes = NULL
 WHERE kind = 'personal'
   AND quota_bytes IS NOT NULL;

-- 2. Resync user-side cached counter to the new sum-of-personal-drives
--    semantics. Mirrors `update_all_users_storage_usage` in
--    `storage_usage_service.rs`. External users excluded (no storage).
UPDATE auth.users u
   SET storage_used_bytes = COALESCE(t.total, 0)
  FROM auth.users u2
  LEFT JOIN (
        SELECT g.subject_id      AS user_id,
               SUM(d.used_bytes)::bigint AS total
          FROM storage.drives      d
          JOIN storage.role_grants g
            ON g.resource_type = 'drive'
           AND g.resource_id   = d.id
           AND g.role          = 'owner'
           AND g.subject_type  = 'user'
         WHERE d.kind = 'personal'
         GROUP BY g.subject_id
       ) t ON t.user_id = u2.id
 WHERE u.id = u2.id
   AND NOT u2.is_external
   AND u.storage_used_bytes IS DISTINCT FROM COALESCE(t.total, 0);
