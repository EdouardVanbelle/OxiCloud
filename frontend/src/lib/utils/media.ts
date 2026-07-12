/** Shared helpers for the photo/video timeline (used by the grid, lightbox,
 * People and Places views). */
import type { FileItem } from '$lib/api/types';

/** True for video tiles (they get a play badge and client-side frame thumbs). */
export function isVideo(p: FileItem): boolean {
	return (p.mime_type ?? '').startsWith('video/');
}

/**
 * EXIF-aware capture timestamp in milliseconds. `sort_date`/`created_at` are
 * stored in seconds; values below ~1e12 are treated as seconds and scaled up.
 */
export function photoTimestamp(p: FileItem): number {
	const v = p.sort_date || p.created_at || 0;
	return v < 1e12 ? v * 1000 : v;
}

/**
 * Build a minimal {@link FileItem} from just an id — used by People and Places
 * to open the lightbox by id and let it lazily fetch the rest (name, EXIF).
 */
export function minimalPhotoItem(id: string): FileItem {
	return {
		category: 'image',
		created_at: 0,
		icon_class: '',
		icon_special_class: '',
		id,
		mime_type: 'image/jpeg',
		modified_at: 0,
		name: '',
		created_by: null,
		updated_by: null,
		folder_id: '',
		path: '',
		size: 0,
		size_formatted: '',
		sort_date: 0,
		etag: '',
		content_hash: ''
	};
}
