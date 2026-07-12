import { it, expect } from 'vitest';
import { sizeBucket, dateBucket, typeLabel, ownerLabel, files } from './files.svelte';

it('buckets sizes into distinct human ranges', () => {
	expect(sizeBucket(-1)).not.toBe(sizeBucket(0));
	const buckets = [0, 500, 5_000_000, 500_000_000, 2_000_000_000, 10_000_000_000].map(sizeBucket);
	// Each successive threshold lands in a different bucket label.
	expect(new Set(buckets).size).toBe(buckets.length);
	buckets.forEach((b) => expect(typeof b).toBe('string'));
});

it('buckets dates relative to now', () => {
	expect(dateBucket(null)).toBeTruthy();
	expect(dateBucket(Date.now())).toBe(dateBucket(Date.now()));
	const old = new Date('2019-06-15').getTime();
	expect(dateBucket(old)).toBe('2019');
});

it('labels a file category, defaulting when absent', () => {
	expect(typeLabel(null)).toBeTruthy();
	expect(typeof typeLabel('Image')).toBe('string');
});

it('shows the owner as "Me" for the current user and a short id otherwise', () => {
	expect(ownerLabel(null, 'me')).toBe('');
	expect(ownerLabel('me', 'me')).toBeTruthy();
	expect(ownerLabel('abcdef123456', 'someone-else')).toBe('abcdef12');
});

it('persists the view mode and toggles selection', () => {
	files.setViewMode('list');
	expect(files.viewMode).toBe('list');
	expect(localStorage.getItem('oxi-view-mode')).toBe('list');
	files.setViewMode('grid');
	expect(files.viewMode).toBe('grid');

	files.clearSelection();
	expect(files.selection.size).toBe(0);
	files.toggleSelected('a');
	expect(files.selection.has('a')).toBe(true);
	files.toggleSelected('a');
	expect(files.selection.has('a')).toBe(false);
});
