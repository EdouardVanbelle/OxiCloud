-- D6: cross-drive folder moves must propagate `drive_id` to the moved
-- folder's subtree (descendant folders + files), not just `lpath`.
--
-- Today's `cascade_folder_path()` trigger only rewrites `path` + `lpath`
-- on descendants — it leaves `drive_id` untouched. That worked when
-- moves were intra-drive (drive_id never changed), but after D5 the
-- `forbid_cross_drive_move` policy gate exposed the gap: a successful
-- cross-drive move (gate off OR not yet enforced) leaves the subtree
-- in an inconsistent state — lpath rooted in drive B but `drive_id`
-- column still drive A on every descendant row. Any drive-id-scoped
-- query then returns the wrong drive's content.
--
-- The fix is to extend the cascade trigger so a change in the parent
-- folder's `drive_id` (the only thing that changes drive_id during a
-- move) cascades to every descendant folder + every descendant file.
-- Files cascade too because `storage.files.drive_id` is the canonical
-- per-file drive-membership signal (D0 dual-write).
--
-- Migration is idempotent via `CREATE OR REPLACE FUNCTION`.

CREATE OR REPLACE FUNCTION storage.cascade_folder_path()
RETURNS trigger AS $$
BEGIN
    IF pg_trigger_depth() > 1 THEN
        RETURN NEW;
    END IF;

    IF OLD.path IS DISTINCT FROM NEW.path OR OLD.lpath IS DISTINCT FROM NEW.lpath THEN
        -- Single batch update: rewrite path/lpath for every descendant
        -- folder at once via the GiST lpath index.
        UPDATE storage.folders
           SET path  = NEW.path || substr(path, length(OLD.path) + 1),
               lpath = NEW.lpath || subpath(lpath, nlevel(OLD.lpath))
         WHERE lpath <@ OLD.lpath
           AND id != NEW.id;
    END IF;

    -- D6: cascade `drive_id` to every descendant folder + file when the
    -- moved row's drive_id has changed (cross-drive move). The GiST
    -- index covers the folder predicate; `storage.files.drive_id` is
    -- updated through the folder→file FK relation since files only
    -- carry `folder_id` directly (drive_id is a denormalised dual-write).
    --
    -- Triggered on the column-list `AFTER UPDATE OF path, lpath, drive_id`
    -- registration below — so this branch only runs when the explicit
    -- move statement on the moved row sets `drive_id` to a new value.
    -- The descendant batch UPDATE that fires from the path/lpath branch
    -- above doesn't touch drive_id, so the trigger doesn't recurse on
    -- the per-descendant rewrite.
    IF OLD.drive_id IS DISTINCT FROM NEW.drive_id THEN
        UPDATE storage.folders
           SET drive_id = NEW.drive_id
         WHERE lpath <@ NEW.lpath
           AND drive_id = OLD.drive_id;

        UPDATE storage.files f
           SET drive_id = NEW.drive_id
          FROM storage.folders fo
         WHERE f.folder_id = fo.id
           AND fo.lpath <@ NEW.lpath
           AND f.drive_id = OLD.drive_id;
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- Re-register the trigger with `drive_id` added to the column list so the
-- trigger fires when a move sets a new drive_id on the moved row. (CREATE
-- OR REPLACE TRIGGER replaces the same name in place; no DROP needed.)
CREATE OR REPLACE TRIGGER trg_folders_cascade_path
    AFTER UPDATE OF path, lpath, drive_id ON storage.folders
    FOR EACH ROW EXECUTE FUNCTION storage.cascade_folder_path();
