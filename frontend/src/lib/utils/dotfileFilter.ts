/**
 * Unix-style dotfile hide convention.
 *
 * A file / folder is considered "hidden" when its display name starts
 * with a `.`. This matches the convention used by every Unix shell,
 * macOS Finder (with Cmd+Shift+.), and every cloud-share product that
 * offers a hide toggle (Nextcloud, ownCloud, Seafile).
 *
 * Windows-style HIDDEN attribute is not honoured — the attribute isn't
 * preserved across upload / dedup, and OxiCloud stores content-
 * addressable blobs without any filesystem metadata carrier. Matches
 * Nextcloud desktop client behaviour, which also strips HIDDEN on
 * upload.
 *
 * Scope: this helper is UI cosmetics ONLY. A direct URL to a hidden
 * file (`/files/<uuid>`) still resolves; batch operations only touch
 * what the UI actually rendered; WebDAV / NC / CalDAV surfaces are
 * unaffected because they consume the raw API responses. The whole
 * filter lives at the render layer, keyed on
 * `preferences.hideDotfiles`.
 */

/** True when the name is a Unix-style hidden file (leading `.`). */
export function isDotfile(name: string): boolean {
	return name.startsWith('.');
}

/**
 * Filter an array of `{ name }`-shaped items down to the visible set.
 * When `hide` is `false`, returns the input array reference unchanged
 * (no allocation, no derived recomputation churn); when `hide` is
 * `true`, returns a new array with dotfiles removed.
 *
 * `T extends { name: string }` matches `FileItem`, `FolderItem`,
 * `SearchHit`, and the mixed `ResourceList` union without further
 * type gymnastics at the call sites.
 */
export function filterDotfiles<T extends { name: string }>(items: T[], hide: boolean): T[] {
	if (!hide) return items;
	return items.filter((item) => !isDotfile(item.name));
}

/**
 * Count the hidden items in an array. Callers use this to render
 * an empty-state hint like "N hidden — show them?" so users don't
 * get surprised by a mysteriously empty folder that actually contains
 * dotfiles.
 */
export function countHidden<T extends { name: string }>(items: T[]): number {
	let n = 0;
	for (const item of items) if (isDotfile(item.name)) n++;
	return n;
}
