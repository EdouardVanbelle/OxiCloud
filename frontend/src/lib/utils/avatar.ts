/**
 * Shared avatar helpers — initials + a deterministic colour bucket — so the
 * user vignette (shares, recipients) and the app-shell account button render
 * identically. Mirrors the original `userVignette` `_initials` / `_colorIndex`.
 */

/** Up-to-two-letter initials from a display label (name or email). */
export function userInitials(label: string | null | undefined): string {
	const base = (label ?? '').trim();
	if (!base) return '?';
	const parts = base.split(/\s+/);
	if (parts.length >= 2) return (parts[0][0] + parts[parts.length - 1][0]).toUpperCase();
	return base.slice(0, 2).toUpperCase();
}

/**
 * Deterministic colour bucket 0–4 from an id, so the same user always gets the
 * same avatar colour (matches the original `userVignette._colorIndex`).
 */
export function avatarColorIndex(id: string | null | undefined): number {
	let hash = 0;
	const s = id ?? '';
	for (let i = 0; i < s.length; i++) hash = (hash * 31 + s.charCodeAt(i)) | 0;
	return Math.abs(hash) % 5;
}
