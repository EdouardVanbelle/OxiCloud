import { it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';

const { confirmDialog, promptDialog } = vi.hoisted(() => ({
	confirmDialog: vi.fn(),
	promptDialog: vi.fn()
}));
vi.mock('$lib/api/endpoints/recent', () => ({ clearRecent: vi.fn(), fetchRecentPage: vi.fn() }));
vi.mock('$lib/api/endpoints/favorites', () => ({
	addFavorite: vi.fn(),
	dateBucket: () => 'Today',
	fetchFavoritesPage: vi.fn(async () => ({ items: [], next_cursor: null })),
	removeFavorite: vi.fn(),
	resolveOwnerName: vi.fn(async () => 'me'),
	sizeBucket: () => 'Small',
	typeLabel: () => 'File'
}));
vi.mock('$lib/api/endpoints/files', () => ({
	fileDownloadUrl: () => '/dl',
	renameFile: vi.fn(),
	deleteFile: vi.fn()
}));
vi.mock('$lib/api/endpoints/folders', () => ({ renameFolder: vi.fn(), deleteFolder: vi.fn() }));
vi.mock('$lib/stores/dialogs.svelte', () => ({ confirmDialog, promptDialog }));

import { fetchRecentPage, clearRecent } from '$lib/api/endpoints/recent';
import { addFavorite } from '$lib/api/endpoints/favorites';
import { deleteFile } from '$lib/api/endpoints/files';
import RecentPage from './+page.svelte';

const m = (fn: unknown) => fn as ReturnType<typeof vi.fn>;

function withOneFile() {
	m(fetchRecentPage).mockResolvedValue({
		items: [
			{
				resource_type: 'file',
				accessed_at: '2024-01-01T00:00:00Z',
				resource: {
					category: 'Document',
					created_at: 0,
					icon_class: 'fa-file',
					icon_special_class: '',
					id: 'r1',
					mime_type: 'text/plain',
					modified_at: 0,
					name: 'notes.txt',
					created_by: 'me',
					updated_by: 'me',
					folder_id: 'root',
					path: '/notes.txt',
					size: 4,
					size_formatted: '4 B',
					sort_date: 0,
					etag: 'e',
					content_hash: 'h'
				}
			}
		],
		next_cursor: null
	});
}

beforeEach(() => vi.clearAllMocks());

it('renders recent items returned by the API', async () => {
	withOneFile();
	render(RecentPage);
	await waitFor(() => expect(fetchRecentPage).toHaveBeenCalled());
	await waitFor(() => expect(screen.getByText('notes.txt')).toBeTruthy());
});

it('clears recent activity after confirmation', async () => {
	withOneFile();
	confirmDialog.mockResolvedValue(true);
	m(clearRecent).mockResolvedValue(undefined);
	render(RecentPage);
	await fireEvent.click(await screen.findByTestId('recent-clear-btn'));
	await waitFor(() => expect(clearRecent).toHaveBeenCalled());
});

it('favorites a recent row via the star button', async () => {
	withOneFile();
	m(addFavorite).mockResolvedValue(undefined);
	render(RecentPage);
	await screen.findByText('notes.txt');
	await fireEvent.click(screen.getByTestId('resource-list-favorite-r1-btn'));
	await waitFor(() => expect(addFavorite).toHaveBeenCalledWith('file', 'r1'));
});

it('batch-deletes selected recent items after confirmation', async () => {
	withOneFile();
	confirmDialog.mockResolvedValue(true);
	m(deleteFile).mockResolvedValue(undefined);
	render(RecentPage);
	await screen.findByText('notes.txt');
	await fireEvent.click(screen.getByTestId('resource-list-select-r1-checkbox'));
	await fireEvent.click(await screen.findByTestId('recent-batch-delete-btn'));
	await waitFor(() => expect(deleteFile).toHaveBeenCalledWith('r1'));
});

it('renders an empty state when there is no recent activity', async () => {
	m(fetchRecentPage).mockResolvedValue({ items: [], next_cursor: null });
	render(RecentPage);
	await waitFor(() => expect(fetchRecentPage).toHaveBeenCalled());
});
