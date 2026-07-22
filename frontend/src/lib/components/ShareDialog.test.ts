import { it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';

const { ui } = vi.hoisted(() => ({ ui: { notify: vi.fn() } }));
vi.mock('$lib/stores/ui.svelte', () => ({ ui }));
vi.mock('$lib/utils/errors', () => ({ errorToast: vi.fn() }));
vi.mock('$lib/api/endpoints/shares', () => ({
	copyShareLink: vi.fn(),
	createShare: vi.fn(),
	deleteShare: vi.fn(),
	listSharesForItem: vi.fn(),
	updateShare: vi.fn()
}));
vi.mock('$lib/api/endpoints/grants', () => ({
	createGrant: vi.fn(),
	expiryToIso: (v: string | null) => v,
	displayRole: (r: string) => r,
	fetchGrantsForResource: vi.fn(),
	notifyGrantRecipient: vi.fn(),
	revokeGrant: vi.fn(),
	todayIso: () => '2026-07-22',
	updateGrantRole: vi.fn()
}));
vi.mock('$lib/api/endpoints/recipients', () => ({
	ensureResolvers: vi.fn(),
	isDirectoryAvailable: () => true,
	resolveRecipient: (_t: string, id: string) => ({ id, label: id }),
	searchRecipients: vi.fn(async () => [])
}));

import { createShare, listSharesForItem } from '$lib/api/endpoints/shares';
import { fetchGrantsForResource } from '$lib/api/endpoints/grants';
import ShareDialog from './ShareDialog.svelte';

const m = (fn: unknown) => fn as ReturnType<typeof vi.fn>;
const item = { id: 'f1', name: 'doc.txt', kind: 'file' as const };

beforeEach(() => {
	vi.clearAllMocks();
	m(fetchGrantsForResource).mockResolvedValue([]);
	m(listSharesForItem).mockResolvedValue([]);
});

it('loads grants and shares when opened', async () => {
	render(ShareDialog, { props: { open: true, item } });
	await screen.findByTestId('share-dialog');
	await waitFor(() => expect(fetchGrantsForResource).toHaveBeenCalledWith('file', 'f1'));
	await waitFor(() => expect(listSharesForItem).toHaveBeenCalledWith('f1', 'file'));
});

it('switches to the link tab and creates a public link', async () => {
	m(createShare).mockResolvedValue({ id: 's1', token: 'abc', has_password: false });
	render(ShareDialog, { props: { open: true, item } });
	await fireEvent.click(await screen.findByTestId('share-dialog-link-tab'));
	await fireEvent.input(screen.getByTestId('share-dialog-link-name-input'), {
		target: { value: 'My link' }
	});
	await fireEvent.click(screen.getByTestId('share-dialog-create-btn'));
	await waitFor(() =>
		expect(createShare).toHaveBeenCalledWith(
			expect.objectContaining({ itemId: 'f1', itemName: 'My link', itemType: 'file' })
		)
	);
});

it('does not load when closed', () => {
	render(ShareDialog, { props: { open: false, item } });
	expect(fetchGrantsForResource).not.toHaveBeenCalled();
});
