import { it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';

const { goto, pageState, session, ui, confirmDialog, promptDialog } = vi.hoisted(() => ({
	goto: vi.fn(),
	pageState: { params: { path: '' }, url: new URL('http://localhost/files') } as {
		params: { path: string };
		url: URL;
	},
	session: {
		user: { id: 'me', username: 'admin', is_external: false },
		isExternalUser: false,
		loadHomeFolder: vi.fn(async () => 'home'),
		refresh: vi.fn(async () => {})
	},
	ui: { notify: vi.fn() },
	confirmDialog: vi.fn(),
	promptDialog: vi.fn()
}));
vi.mock('$app/navigation', () => ({ goto }));
vi.mock('$app/state', () => ({ page: pageState }));
vi.mock('$lib/stores/session.svelte', () => ({ session }));
vi.mock('$lib/stores/ui.svelte', () => ({ ui }));
vi.mock('$lib/stores/dialogs.svelte', () => ({ confirmDialog, promptDialog }));
vi.mock('$lib/api/client', () => ({ apiFetch: vi.fn() }));
vi.mock('$lib/api/csrf', () => ({ getCsrfHeaders: () => ({}) }));
vi.mock('$lib/api/endpoints/deltaUpload', () => ({ tryDeltaUpload: vi.fn() }));
vi.mock('$lib/api/endpoints/favorites', () => ({ addFavorite: vi.fn(), removeFavorite: vi.fn() }));
vi.mock('$lib/api/endpoints/wopi', () => ({
	canEditWithWopi: () => false,
	getEditorUrlWithFallback: vi.fn()
}));
vi.mock('$lib/api/endpoints/music', () => ({
	addTracks: vi.fn(),
	createPlaylist: vi.fn(),
	listPlaylists: vi.fn(async () => [])
}));
vi.mock('$lib/api/endpoints/files', () => ({
	deleteFile: vi.fn(),
	fileDownloadUrl: () => '/dl',
	fileThumbnailUrl: () => '/thumb',
	moveFile: vi.fn(),
	renameFile: vi.fn(),
	uploadFile: vi.fn(),
	uploadFileWithProgress: vi.fn()
}));
vi.mock('$lib/api/endpoints/folders', () => ({
	cacheFolder: vi.fn(),
	createFolder: vi.fn(),
	deleteFolder: vi.fn(),
	fetchFolderListing: vi.fn(),
	folderZipUrl: () => '/zip',
	getCachedFolder: () => undefined,
	getFolder: vi.fn(async (id: string) => ({ id, name: id })),
	getFolderName: () => undefined,
	invalidateFolderCache: vi.fn(),
	moveFolder: vi.fn(),
	rememberFolderName: vi.fn(),
	renameFolder: vi.fn()
}));

import { fetchFolderListing, createFolder, deleteFolder } from '$lib/api/endpoints/folders';
import { deleteFile } from '$lib/api/endpoints/files';
import { apiFetch } from '$lib/api/client';
import { files as filesStore } from '$lib/stores/files.svelte';
import FilesPage from './[...path]/+page.svelte';

const m = (fn: unknown) => fn as ReturnType<typeof vi.fn>;

function withListing() {
	m(fetchFolderListing).mockResolvedValue({
		status: 200,
		etag: 'v1',
		listing: {
			folders: [folderItem('sub1', 'Sub')],
			files: [fileItem('f1', 'hello.txt')],
			favoriteIds: [],
			sharedIds: []
		}
	});
}

function fileItem(id: string, name: string) {
	return {
		category: 'Document',
		created_at: 0,
		icon_class: 'fa-file',
		icon_special_class: '',
		id,
		mime_type: 'text/plain',
		modified_at: 0,
		name,
		created_by: 'me',
		updated_by: 'me',
		folder_id: 'home',
		path: '/' + name,
		size: 4,
		size_formatted: '4 B',
		sort_date: 0,
		etag: 'e',
		content_hash: 'h'
	};
}
function folderItem(id: string, name: string) {
	return {
		category: 'Folder',
		created_at: 0,
		icon_class: 'fa-folder',
		icon_special_class: '',
		id,
		is_root: false,
		modified_at: 0,
		name,
		created_by: 'me',
		updated_by: 'me',
		parent_id: 'home',
		path: '/' + name,
		etag: 'e'
	};
}

beforeEach(() => {
	vi.clearAllMocks();
	// A concrete folder in the path: bare `/files` now canonicalizes to
	// `/files/<drive-root>` via goto (see the external-user test), so the
	// listing-oriented tests target a folder directly.
	pageState.params.path = 'home';
	// List view renders the select-all header + per-row checkboxes; grid hides them.
	filesStore.viewMode = 'list';
});

it('loads the home folder listing on mount and renders its contents', async () => {
	m(fetchFolderListing).mockResolvedValue({
		status: 200,
		etag: 'v1',
		listing: {
			folders: [folderItem('sub1', 'Sub')],
			files: [fileItem('f1', 'hello.txt')],
			favoriteIds: [],
			sharedIds: []
		}
	});
	render(FilesPage);
	await waitFor(() => expect(fetchFolderListing).toHaveBeenCalledWith('home', expect.anything()));
	// VirtualList windows rows by viewport height (0 in jsdom), so assert the
	// surrounding chrome rendered rather than the windowed rows themselves.
	await screen.findByTestId('files-new-folder-btn');
});

it('shows an error when the listing fails with no cache', async () => {
	m(fetchFolderListing).mockRejectedValue(Object.assign(new Error('nope'), { status: 500 }));
	render(FilesPage);
	await waitFor(() => expect(fetchFolderListing).toHaveBeenCalled());
});

it('redirects external users away from the home folder', async () => {
	session.isExternalUser = true;
	pageState.params.path = '';
	render(FilesPage);
	await waitFor(() => expect(goto).toHaveBeenCalledWith('/shared-with-me', { replaceState: true }));
	session.isExternalUser = false;
});

it('creates a new folder in the current directory', async () => {
	withListing();
	promptDialog.mockResolvedValue('Reports');
	m(createFolder).mockResolvedValue({ id: 'new', name: 'Reports' });
	render(FilesPage);
	await fireEvent.click(await screen.findByTestId('files-new-folder-btn'));
	await waitFor(() => expect(createFolder).toHaveBeenCalledWith('Reports', 'home'));
});

it('batch-deletes the whole selection after confirmation', async () => {
	withListing();
	confirmDialog.mockResolvedValue(true);
	m(deleteFolder).mockResolvedValue(undefined);
	m(deleteFile).mockResolvedValue(undefined);
	render(FilesPage);
	await fireEvent.click(await screen.findByTestId('files-select-all-checkbox'));
	await fireEvent.click(await screen.findByTestId('files-batch-delete-btn'));
	await waitFor(() => expect(deleteFolder).toHaveBeenCalledWith('sub1'));
	await waitFor(() => expect(deleteFile).toHaveBeenCalledWith('f1'));
});

it('batch-favorites the selection via the favorites batch endpoint', async () => {
	withListing();
	m(apiFetch).mockResolvedValue({ ok: true });
	render(FilesPage);
	await fireEvent.click(await screen.findByTestId('files-select-all-checkbox'));
	await fireEvent.click(await screen.findByTestId('files-batch-favorite-btn'));
	await waitFor(() =>
		expect(apiFetch).toHaveBeenCalledWith(
			'/api/favorites/batch',
			expect.objectContaining({ method: 'POST' })
		)
	);
});
