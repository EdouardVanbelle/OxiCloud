-- Drop the pre-Round-3 per-domain share tables. Every reader/writer
-- was retired in the Rust cleanup landing alongside this migration:
--
--   * `CalendarUseCase::{list_shared_calendars, share_calendar,
--     remove_calendar_sharing, get_calendar_shares}` — gone
--   * `AddressBookUseCase::{share_address_book, unshare_address_book,
--     get_address_book_shares}` — gone
--   * `CalendarRepository` / `AddressBookRepository` share methods — gone
--   * SQL bodies in `calendar_pg_repository.rs` /
--     `address_book_pg_repository.rs` that touched these tables — gone
--
-- Data lives on in `storage.role_grants` (backfilled by
-- `20260906000001_backfill_calendar_address_book_role_grants.sql`).
-- The one-release rollback window between the backfill and this drop
-- was left implicit — no external process reads either table today.

DROP TABLE IF EXISTS caldav.calendar_shares;
DROP TABLE IF EXISTS carddav.address_book_shares;

-- Post-flight introspection: refuse to complete if either table is
-- still present. Guards against a name-collision resurrection by an
-- older seed file or hand-rolled restore step.
DO $$
DECLARE
    stray_count INT;
BEGIN
    SELECT COUNT(*) INTO stray_count
    FROM pg_class c
    JOIN pg_namespace n ON n.oid = c.relnamespace
    WHERE (n.nspname = 'caldav'  AND c.relname = 'calendar_shares')
       OR (n.nspname = 'carddav' AND c.relname = 'address_book_shares');

    IF stray_count > 0 THEN
        RAISE EXCEPTION
            'Migration 20260906000002 finished with % legacy share table(s) still present',
            stray_count;
    END IF;
END $$;
