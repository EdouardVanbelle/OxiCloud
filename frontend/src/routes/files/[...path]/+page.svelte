<script lang="ts">
	import SkeletonList from '$lib/components/SkeletonList.svelte';
	import EmptyState from '$lib/components/EmptyState.svelte';
	import { errorMessage, errorToast } from '$lib/utils/errors';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { untrack } from 'svelte';
	import Icon from '$lib/icons/Icon.svelte';
	import {
		cacheFolder,
		createFolder,
		deleteFolder,
		fetchFolderListing,
		getCachedFolder,
		getFolder,
		getFolderName,
		invalidateFolderCache,
		moveFolder,
		rememberFolderName,
		renameFolder,
		type FolderListing
	} from '$lib/api/endpoints/folders';
	import {
		deleteFile,
		fileDownloadUrl,
		fileThumbnailUrl,
		moveFile,
		renameFile,
		uploadFile,
		uploadFileWithProgress
	} from '$lib/api/endpoints/files';
	import { folderZipUrl } from '$lib/api/endpoints/folders';
	import { tryDeltaUpload } from '$lib/api/endpoints/deltaUpload';
	import { addFavorite, removeFavorite } from '$lib/api/endpoints/favorites';
	import { canEditWithWopi, getEditorUrlWithFallback } from '$lib/api/endpoints/wopi';
	import { addTracks, createPlaylist, listPlaylists } from '$lib/api/endpoints/music';
	import { apiFetch } from '$lib/api/client';
	import { getCsrfHeaders } from '$lib/api/csrf';
	import type { FileItem, FolderItem, ItemType } from '$lib/api/types';
	import ListToolbar from '$lib/components/ListToolbar.svelte';
	import VirtualList from '$lib/components/VirtualList.svelte';
	import MoveDialog from '$lib/components/MoveDialog.svelte';
	import ShareDialog from '$lib/components/ShareDialog.svelte';
	import { lazyComponent } from '$lib/composables/lazyComponent.svelte';
	import { t } from '$lib/i18n/index.svelte';
	import { confirmDialog, promptDialog } from '$lib/stores/dialogs.svelte';
	import { files as filesStore } from '$lib/stores/files.svelte';
	import { session } from '$lib/stores/session.svelte';
	import { ui } from '$lib/stores/ui.svelte';
	import {
		dateBucket,
		ownerLabel,
		relativeTimeAgo,
		sizeBucket,
		typeLabel
	} from '$lib/stores/files.svelte';
	import { formatBytes } from '$lib/utils/format';
	import { formatDate, iconNameFromClass } from '$lib/utils/display';
	import { gridColumns } from '$lib/utils/grid';

	// File preview and the WOPI editor are heavy and only appear on demand, so
	// their modules load the first time the user opens one (see the effects that
	// call `.load()` when `viewerOpen` / `wopiOpen` flip true).
	const fileViewer = lazyComponent(() => import('$lib/components/FileViewer.svelte'));
	const wopiEditor = lazyComponent(() => import('$lib/components/WopiEditor.svelte'));

	// The URL rest param is the trail of folder ids from home's children down.
	// /files → home root; /files/a/b → folder b inside a inside home.
	const pathSegments = $derived((page.params.path ?? '').split('/').filter((s) => s.length > 0));

	let listing = $state<FolderListing>({ folders: [], files: [], favoriteIds: [], sharedIds: [] });
	let crumbs = $state<Array<{ id: string; name: string }>>([]);
	let currentId = $state<string | null>(null);
	let loading = $state(false);
	// Skeleton is delayed ~100ms behind `loading` so fast loads don't flash it.
	let showSkeleton = $state(false);
	let error = $state<string | null>(null);
	let fileInput = $state<HTMLInputElement | null>(null);
	let uploading = $state(false);
	let dragOver = $state(false);

	interface ActionTarget {
		id: string;
		name: string;
		kind: ItemType;
	}
	let moveOpen = $state(false);
	let moveMode = $state<'move' | 'copy'>('move');
	let shareOpen = $state(false);
	let actionTarget = $state<ActionTarget | null>(null);
	let moveItems = $state<ActionTarget[] | null>(null);

	// Favorite + shared badge sets for the current folder, seeded directly from
	// the listing response (server-computed, scoped to these items — no extra
	// per-navigation fetch) and updated optimistically on mutation.
	let favoriteIds = $state<Set<string>>(new Set());
	let sharedIds = $state<Set<string>>(new Set());

	function openMove(kind: ItemType, id: string, name: string) {
		actionTarget = { id, name, kind };
		moveItems = null;
		moveMode = 'move';
		moveOpen = true;
	}
	function openCopy(kind: ItemType, id: string, name: string) {
		actionTarget = { id, name, kind };
		moveItems = null;
		moveMode = 'copy';
		moveOpen = true;
	}
	function openShare(kind: ItemType, id: string, name: string) {
		actionTarget = { id, name, kind };
		shareOpen = true;
	}

	async function toggleFavorite(kind: ItemType, id: string) {
		const isFav = favoriteIds.has(id);
		// Optimistic toggle, reverted on failure.
		const next = new Set(favoriteIds);
		if (isFav) next.delete(id);
		else next.add(id);
		favoriteIds = next;
		try {
			if (isFav) await removeFavorite(kind, id);
			else await addFavorite(kind, id);
		} catch (e) {
			errorToast(e);
			const reverted = new Set(favoriteIds);
			if (isFav) reverted.add(id);
			else reverted.delete(id);
			favoriteIds = reverted;
		}
	}

	async function buildCrumbs(segments: string[]): Promise<Array<{ id: string; name: string }>> {
		// Names come from the cache first (every listing names its children, so
		// step-by-step navigation needs zero requests); only ids we've never seen
		// — a cold deep-link's ancestors — are fetched, in parallel.
		return Promise.all(
			segments.map(async (id) => {
				const known = getFolderName(id);
				if (known !== undefined) return { id, name: known };
				try {
					const f = await getFolder(id);
					return { id, name: f.name };
				} catch {
					return { id, name: '…' };
				}
			})
		);
	}

	// Bumped on every load; a stale in-flight response checks this before it
	// writes state, so a fast navigation can't be clobbered by an older fetch.
	let loadSeq = 0;

	function applyListing(data: FolderListing) {
		listing = data;
		favoriteIds = new Set(data.favoriteIds);
		sharedIds = new Set(data.sharedIds);
	}

	async function load() {
		error = null;
		const seq = ++loadSeq;

		// External users have no home folder; send them to shared-with-me.
		if (session.isExternalUser && pathSegments.length === 0) {
			await goto('/shared-with-me', { replaceState: true });
			return;
		}
		const home = await session.loadHomeFolder();
		const folderId = pathSegments.at(-1) ?? home;
		if (!folderId) {
			error = t('files.no_home', 'No home folder available.');
			return;
		}
		currentId = folderId;
		filesStore.currentFolder = folderId;

		// Stale-while-revalidate: paint a previously-visited folder instantly,
		// then revalidate with If-None-Match (304 = keep what's shown).
		const cached = getCachedFolder(folderId);
		if (cached) {
			applyListing(cached.listing);
			loading = false;
			showSkeleton = false;
		} else {
			loading = true;
		}
		// Delayed skeleton, only when there's nothing cached to show yet.
		const skeletonTimer = setTimeout(() => {
			if (loading) showSkeleton = true;
		}, 100);

		// Breadcrumbs resolve independently so they never block the grid paint.
		void buildCrumbs(pathSegments).then((trail) => {
			if (seq === loadSeq) crumbs = trail;
		});

		try {
			const res = await fetchFolderListing(folderId, { etag: cached?.etag });
			if (seq !== loadSeq) return; // superseded by a newer navigation
			if (res.status === 200 && res.listing) {
				applyListing(res.listing);
				cacheFolder(folderId, res.listing, res.etag);
			}
			// 304 → the cached copy already on screen is current.
			error = null;
		} catch (e) {
			if (seq !== loadSeq) return;
			// With a cached view already shown, keep it on a transient failure.
			if (!cached) {
				const status = (e as { status?: number })?.status;
				error =
					status === 403
						? t('errors.forbidden', 'Could not load files')
						: e instanceof Error
							? e.message
							: String(e);
			}
		} finally {
			clearTimeout(skeletonTimer);
			if (seq === loadSeq) {
				loading = false;
				showSkeleton = false;
			}
		}
	}

	/** Data changed — drop cached listings and reload the current folder fresh. */
	async function reload() {
		invalidateFolderCache();
		await load();
	}

	function openFolder(folder: FolderItem) {
		goto(`/files/${[...pathSegments, folder.id].join('/')}`);
	}

	function crumbHref(index: number): string {
		return `/files/${pathSegments.slice(0, index + 1).join('/')}`;
	}

	async function onNewFolder() {
		const name = await promptDialog({
			title: t('files.new_folder', 'New folder'),
			placeholder: t('files.new_folder_prompt', 'New folder name'),
			confirmText: t('common.create', 'Create')
		});
		if (!name) return;
		try {
			await createFolder(name, currentId);
			await reload();
		} catch (e) {
			errorToast(e);
		}
	}

	/**
	 * Upload a batch of files into the current folder, reporting aggregate
	 * progress through a single bell notification with a progress bar.
	 */
	async function uploadBatch(files: File[]) {
		if (files.length === 0) return;
		uploading = true;
		const total = files.length;
		const label = (done: number) =>
			total === 1
				? t('files.uploading_file', { name: files[0].name }, `Uploading ${files[0].name}…`)
				: t('files.uploading_n', { done, total }, `Uploading ${done}/${total} files…`);
		const nid = ui.startProgress(label(0));
		let savedBytes = 0;
		try {
			for (let i = 0; i < files.length; i++) {
				const report = (frac: number) => {
					const base = i / total;
					const step = Number.isNaN(frac) ? 0 : frac / total;
					ui.updateProgress(nid, Math.round((base + step) * 100), label(i));
				};
				// Delta upload for large files (dedup); transparently falls back.
				const delta = await tryDeltaUpload(files[i], currentId, (pct) => report(pct / 100));
				if (delta) {
					if (!delta.ok) throw new Error(delta.errorMsg ?? 'upload failed');
					savedBytes += delta.savedBytes ?? 0;
				} else {
					await uploadFileWithProgress(currentId, files[i], report);
				}
				ui.updateProgress(nid, Math.round(((i + 1) / total) * 100), label(i + 1));
			}
			const done =
				savedBytes > 0
					? t(
							'files.uploaded_saved',
							{ mb: (savedBytes / (1024 * 1024)).toFixed(1) },
							`Upload complete — ${(savedBytes / (1024 * 1024)).toFixed(1)} MB deduplicated`
						)
					: t('files.uploaded', 'Upload complete');
			ui.finishProgress(nid, done, 'success');
			await reload();
		} catch (err) {
			ui.finishProgress(nid, errorMessage(err), 'error');
		} finally {
			uploading = false;
		}
	}

	async function onUpload(e: Event) {
		const input = e.target as HTMLInputElement;
		if (!input.files?.length) return;
		await uploadBatch(Array.from(input.files));
		input.value = '';
	}

	async function onDrop(e: DragEvent) {
		e.preventDefault();
		dragOver = false;
		const dt = e.dataTransfer;
		if (!dt) return;
		// A dropped folder isn't expanded into `.files`, so walk the dropped entry
		// tree (webkitGetAsEntry) when one is present and recreate it server-side;
		// otherwise fall back to the flat file list.
		const tree = await collectDroppedEntries(dt);
		if (tree) await uploadTree(tree);
		else if (dt.files?.length) await uploadBatch(Array.from(dt.files));
	}

	/**
	 * Expand dropped OS entries into `{file, relativePath}` rows, walking any
	 * directory tree via the (non-standard but ubiquitous) `webkitGetAsEntry` /
	 * `createReader` API. Returns `null` when nothing dropped was a directory, so
	 * the caller takes the simpler flat-`FileList` path.
	 */
	async function collectDroppedEntries(
		dt: DataTransfer
	): Promise<{ file: File; relativePath: string }[] | null> {
		// `webkitGetAsEntry()` must be read synchronously while the event is live.
		const roots: FileSystemEntry[] = [];
		let sawDir = false;
		for (const item of Array.from(dt.items)) {
			const entry = item.webkitGetAsEntry();
			if (entry) {
				roots.push(entry);
				if (entry.isDirectory) sawDir = true;
			}
		}
		if (!sawDir) return null;

		const out: { file: File; relativePath: string }[] = [];
		async function walk(entry: FileSystemEntry, prefix: string): Promise<void> {
			if (entry.isFile) {
				const file = await new Promise<File>((resolve, reject) =>
					(entry as FileSystemFileEntry).file(resolve, reject)
				);
				out.push({ file, relativePath: prefix + entry.name });
			} else if (entry.isDirectory) {
				const reader = (entry as FileSystemDirectoryEntry).createReader();
				const dirPrefix = `${prefix}${entry.name}/`;
				// readEntries yields in batches; loop until it returns an empty one.
				for (;;) {
					const batch = await new Promise<FileSystemEntry[]>((resolve, reject) =>
						reader.readEntries(resolve, reject)
					);
					if (batch.length === 0) break;
					for (const child of batch) await walk(child, dirPrefix);
				}
			}
		}
		for (const root of roots) await walk(root, '');
		return out;
	}

	async function renameItem(kind: 'file' | 'folder', id: string, current: string) {
		const name = await promptDialog({
			title: t('common.rename', 'Rename'),
			defaultValue: current,
			confirmText: t('common.save', 'Save')
		});
		if (!name || name === current) return;
		try {
			if (kind === 'file') await renameFile(id, name);
			else {
				await renameFolder(id, name);
				rememberFolderName(id, name); // keep breadcrumbs current immediately
			}
			await reload();
		} catch (e) {
			errorToast(e);
		}
	}

	async function deleteItem(kind: 'file' | 'folder', id: string, name: string) {
		const ok = await confirmDialog({
			title: t('common.delete', 'Delete'),
			message: t('files.confirm_delete', { name }, 'Move "{{name}}" to trash?'),
			confirmText: t('common.delete', 'Delete'),
			danger: true
		});
		if (!ok) return;
		try {
			if (kind === 'file') await deleteFile(id);
			else await deleteFolder(id);
			await reload();
		} catch (e) {
			errorToast(e);
		}
	}

	let viewerOpen = $state(false);
	let viewerFile = $state<FileItem | null>(null);

	function openFile(file: FileItem) {
		// Drive the viewer from the URL (?file=<id>) so a preview is bookmarkable,
		// reload-restorable, and Back/Forward open/close it. The effect below
		// reflects the param into viewerOpen/viewerFile.
		const url = new URL(page.url);
		url.searchParams.set('file', file.id);
		void goto(url, { keepFocus: true, noScroll: true });
	}

	// ── File-preview deep link (?file=<id>) ──────────────────────────────────
	// URL → viewer. Runs on navigation, on popstate (Back/Forward), and once the
	// listing for an initial deep link arrives. `untrack` stops it re-firing on
	// viewer-state changes, so a user-initiated close can't be re-opened here.
	$effect(() => {
		const fileId = page.url.searchParams.get('file');
		const files = listing.files;
		untrack(() => {
			if (!fileId) {
				if (viewerOpen) viewerOpen = false;
				return;
			}
			if (viewerOpen && viewerFile?.id === fileId) return;
			const f = files.find((x) => x.id === fileId);
			if (f) {
				viewerFile = f;
				viewerOpen = true;
			}
		});
	});

	// viewer → URL: when closed from within (X / Esc / backdrop), drop the param
	// (replaceState, so closing doesn't add a history entry).
	$effect(() => {
		const hasParam = page.url.searchParams.get('file') !== null;
		if (!viewerOpen && hasParam) {
			const url = new URL(page.url);
			url.searchParams.delete('file');
			void goto(url, { keepFocus: true, noScroll: true, replaceState: true });
		}
	});

	/**
	 * Whether the server can render a thumbnail preview for this file. Images and
	 * videos always have one; PDFs (and other thumbnail-capable docs) do too, so
	 * surface those rather than a generic icon. A failed <img> load falls back to
	 * the icon via onerror, so being permissive here is safe.
	 */
	function canThumbnail(file: FileItem): boolean {
		const m = file.mime_type ?? '';
		if (m.startsWith('image/') || m.startsWith('video/')) return true;
		if (m === 'application/pdf') return true;
		return /\.pdf$/i.test(file.name);
	}

	// ── Multi-select + batch ────────────────────────────────────────────────
	let selected = $state<Set<string>>(new Set());
	// Anchor row id for shift-click range selection.
	let selectionAnchor = $state<string | null>(null);

	function toggleSelected(id: string) {
		const next = new Set(selected);
		if (next.has(id)) next.delete(id);
		else next.add(id);
		selected = next;
		selectionAnchor = id;
	}
	function clearSelection() {
		selected = new Set();
		selectionAnchor = null;
	}

	/**
	 * Row click selection mirroring static/js/components/resourceList.js:
	 *  - Shift+click selects the contiguous range from the anchor to this row.
	 *  - Ctrl/Cmd+click toggles this row without clearing the rest.
	 *  - A plain click (no modifier) opens the item — handled by the caller.
	 * Returns true when the click was consumed as a selection gesture.
	 */
	function handleSelectionClick(e: MouseEvent, id: string): boolean {
		if (e.shiftKey && selectionAnchor) {
			e.preventDefault();
			const a = orderedIds.indexOf(selectionAnchor);
			const b = orderedIds.indexOf(id);
			if (a !== -1 && b !== -1) {
				const [lo, hi] = a < b ? [a, b] : [b, a];
				selected = new Set([...selected, ...orderedIds.slice(lo, hi + 1)]);
			}
			return true;
		}
		if (e.ctrlKey || e.metaKey) {
			e.preventDefault();
			toggleSelected(id);
			return true;
		}
		return false;
	}

	const selectedCount = $derived(selected.size);
	const totalCount = $derived(listing.folders.length + listing.files.length);

	function toggleSelectAll() {
		if (selected.size === totalCount) clearSelection();
		else selected = new Set([...listing.folders, ...listing.files].map((i) => i.id));
	}

	/**
	 * Download the whole selection as a single zip via POST /api/batch/download —
	 * folders are included (the old per-item loop silently skipped them). A lone
	 * file still streams directly so it keeps its original name/extension.
	 */
	/** Name for a server-zipped multi-item archive (matches the legacy format). */
	function batchZipName(): string {
		const stamp = new Date().toISOString().replace('T', ' ').replace(/\..*/, '').replace(/:/g, '-');
		return `oxicloud ${stamp}.zip`;
	}

	async function batchDownload() {
		const fileIds: string[] = [];
		const folderIds: string[] = [];
		for (const id of selected) {
			if (listing.folders.some((f) => f.id === id)) folderIds.push(id);
			else if (listing.files.some((f) => f.id === id)) fileIds.push(id);
		}
		if (fileIds.length === 0 && folderIds.length === 0) return;

		// Single file, no folders → direct download (preserves the real name).
		if (fileIds.length === 1 && folderIds.length === 0) {
			const file = listing.files.find((f) => f.id === fileIds[0]);
			if (file) {
				const a = document.createElement('a');
				a.href = fileDownloadUrl(file.id);
				a.download = file.name;
				document.body.appendChild(a);
				a.click();
				a.remove();
			}
			return;
		}

		const zipName = batchZipName();
		try {
			const res = await apiFetch('/api/batch/download', {
				method: 'POST',
				credentials: 'same-origin',
				headers: { 'Content-Type': 'application/json', ...getCsrfHeaders() },
				body: JSON.stringify({ file_ids: fileIds, folder_ids: folderIds })
			});
			if (!res.ok) throw new Error(`Server returned ${res.status}`);
			const blob = await res.blob();
			const url = URL.createObjectURL(blob);
			const a = document.createElement('a');
			a.href = url;
			a.download = zipName;
			document.body.appendChild(a);
			a.click();
			a.remove();
			URL.revokeObjectURL(url);
		} catch (e) {
			errorToast(e);
		}
	}

	/** Batch add the selection to favorites — single /api/favorites/batch call. */
	async function batchFavorites() {
		const items = selectionTargets().filter((it) => !favoriteIds.has(it.id));
		if (items.length === 0) {
			ui.notify(t('files.already_favorites', 'All selected items are already favorites'), 'info');
			clearSelection();
			return;
		}
		try {
			const res = await apiFetch('/api/favorites/batch', {
				method: 'POST',
				credentials: 'same-origin',
				headers: { 'Content-Type': 'application/json', ...getCsrfHeaders() },
				body: JSON.stringify({
					items: items.map((it) => ({ item_id: it.id, item_type: it.kind }))
				})
			});
			if (!res.ok) throw new Error(`Server returned ${res.status}`);
			favoriteIds = new Set([...favoriteIds, ...items.map((it) => it.id)]);
			ui.notify(t('files.added_favorites', 'Added to favorites'), 'success');
			clearSelection();
		} catch (e) {
			errorToast(e);
		}
	}

	function selectionTargets(): ActionTarget[] {
		return [...selected]
			.map((id) => {
				const folder = listing.folders.find((f) => f.id === id);
				if (folder) return { id, name: folder.name, kind: 'folder' as ItemType };
				const file = listing.files.find((f) => f.id === id);
				return file ? { id, name: file.name, kind: 'file' as ItemType } : null;
			})
			.filter((x): x is ActionTarget => x !== null);
	}

	function batchMove() {
		const items = selectionTargets();
		if (items.length) {
			moveItems = items;
			moveMode = 'move';
			moveOpen = true;
		}
	}

	function batchCopy() {
		const items = selectionTargets();
		if (items.length) {
			moveItems = items;
			moveMode = 'copy';
			moveOpen = true;
		}
	}

	function onKeydown(e: KeyboardEvent) {
		const tag = (e.target as HTMLElement)?.tagName;
		if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return;
		if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'a') {
			e.preventDefault();
			toggleSelectAll();
		} else if (e.key === 'Escape' && selected.size) {
			clearSelection();
		} else if (e.key === 'Delete' && selected.size) {
			// Delete only — Backspace was dropped: it triggered accidental deletes.
			e.preventDefault();
			void batchDelete();
		}
	}

	async function batchDelete() {
		const ids = [...selected];
		const ok = await confirmDialog({
			title: t('files.batch_delete', 'Delete selected'),
			message: t('files.confirm_batch_delete', { n: ids.length }, 'Move {{n}} items to trash?'),
			confirmText: t('common.delete', 'Delete'),
			danger: true
		});
		if (!ok) return;
		for (const id of ids) {
			const folder = listing.folders.find((f) => f.id === id);
			try {
				if (folder) await deleteFolder(id);
				else await deleteFile(id);
			} catch (e) {
				errorToast(e);
			}
		}
		clearSelection();
		await reload();
	}

	// ── Drag-to-move ─────────────────────────────────────────────────────────
	const DRAG_TYPE = 'application/x-oxi-item';
	let dropFolderId = $state<string | null>(null);

	/**
	 * Begin dragging an item. When the dragged row is part of the current
	 * selection, the whole selection travels (mirrors ui.js' multi-item drag);
	 * otherwise just the single item moves.
	 */
	function onItemDragStart(e: DragEvent, kind: ItemType, id: string, name: string) {
		const items: ActionTarget[] =
			selected.has(id) && selected.size > 1 ? selectionTargets() : [{ id, name, kind }];
		e.dataTransfer?.setData(DRAG_TYPE, JSON.stringify(items));
		if (e.dataTransfer) {
			e.dataTransfer.effectAllowed = 'move';
			if (items.length > 1) showDragGhost(e.dataTransfer, items);
			// Drag-out-to-OS download: the OS reads `DownloadURL` (a GET URL) and
			// downloads the dragged item(s) — a single file directly, a folder or a
			// multi-selection as one server-zipped archive.
			const dl = dragDownloadDescriptor(items);
			if (dl) {
				e.dataTransfer.setData(
					'DownloadURL',
					`application/octet-stream:${dl.name}:${location.origin}${dl.url}`
				);
			}
		}
	}

	/** `{ name, GET url }` for the drag-out download of the current drag set. */
	function dragDownloadDescriptor(items: ActionTarget[]): { name: string; url: string } | null {
		if (items.length === 0) return null;
		if (items.length === 1) {
			const it = items[0];
			return it.kind === 'folder'
				? { name: `${it.name}.zip`, url: folderZipUrl(it.id) }
				: { name: it.name, url: fileDownloadUrl(it.id) };
		}
		// Multi-selection → one archive via the GET twin of POST /api/batch/download
		// (DownloadURL can only point at a GET URL); file_ids/folder_ids are CSV.
		const fileIds = items.filter((i) => i.kind === 'file').map((i) => i.id);
		const folderIds = items.filter((i) => i.kind === 'folder').map((i) => i.id);
		const params = new URLSearchParams();
		if (fileIds.length) params.set('file_ids', fileIds.join(','));
		if (folderIds.length) params.set('folder_ids', folderIds.join(','));
		return { name: batchZipName(), url: `/api/batch/download?${params.toString()}` };
	}

	/**
	 * Custom drag image for a multi-item drag: a stack of the first few rows plus
	 * a count badge (ported from ui.js). Reuses the .drag-preview / .dragged-items
	 * / .dragged-items-badge styles already in resourceList.css.
	 */
	function showDragGhost(dt: DataTransfer, items: ActionTarget[]) {
		const MAX = 4;
		const preview = document.createElement('div');
		preview.className = 'drag-preview';

		const stack = document.createElement('div');
		stack.className = 'dragged-items';
		for (const [i, it] of items.slice(0, MAX).entries()) {
			const row = document.createElement('div');
			row.className = 'file-item';
			if (i === MAX - 1 && items.length > MAX) row.classList.add('fading');
			const icon = document.createElement('div');
			icon.className = 'file-icon';
			icon.textContent = it.kind === 'folder' ? '📁' : '📄';
			const label = document.createElement('div');
			label.textContent = it.name;
			row.append(icon, label);
			stack.appendChild(row);
		}

		const badge = document.createElement('div');
		badge.className = 'dragged-items-badge';
		badge.textContent = String(items.length);

		preview.append(stack, badge);
		document.body.appendChild(preview);
		dt.setDragImage(preview, 0, 0);
		// The browser snapshots the drag image synchronously; drop the node next tick.
		setTimeout(() => preview.remove(), 0);
	}

	function dragPayload(e: DragEvent): ActionTarget[] {
		const raw = e.dataTransfer?.getData(DRAG_TYPE);
		if (!raw) return [];
		try {
			const parsed = JSON.parse(raw);
			return Array.isArray(parsed) ? parsed : [parsed];
		} catch {
			return [];
		}
	}

	async function moveInto(targetFolderId: string, e: DragEvent) {
		const items = dragPayload(e).filter((it) => it.id !== targetFolderId);
		if (items.length === 0) return;
		try {
			for (const it of items) {
				if (it.kind === 'file') await moveFile(it.id, targetFolderId);
				else await moveFolder(it.id, targetFolderId);
			}
			clearSelection();
			await reload();
		} catch (err) {
			errorToast(err);
		}
	}

	function onFolderDrop(e: DragEvent, folder: FolderItem) {
		if (!e.dataTransfer?.types.includes(DRAG_TYPE)) return; // external file drop → page dropzone
		e.preventDefault();
		e.stopPropagation();
		dropFolderId = null;
		void moveInto(folder.id, e);
	}

	function onCrumbDrop(e: DragEvent, folderId: string) {
		if (!e.dataTransfer?.types.includes(DRAG_TYPE)) return;
		e.preventDefault();
		void moveInto(folderId, e);
	}

	// ── Right-click context menu ─────────────────────────────────────────────
	let ctxOpen = $state(false);
	let ctxX = $state(0);
	let ctxY = $state(0);
	let ctxTarget = $state<ActionTarget | null>(null);

	function openContext(e: MouseEvent, kind: ItemType, id: string, name: string) {
		e.preventDefault();
		e.stopPropagation();
		ctxTarget = { id, name, kind };
		// Clamp to viewport so the menu never overflows offscreen.
		ctxX = Math.min(e.clientX, window.innerWidth - 200);
		ctxY = Math.min(e.clientY, window.innerHeight - 320);
		ctxOpen = true;
	}
	function closeContext() {
		ctxOpen = false;
		ctxTarget = null;
	}

	// ── WOPI office editor (context-menu entries) ────────────────────────────
	let wopiOpen = $state(false);
	let wopiAction = $state<'edit' | 'view'>('edit');
	let wopiFile = $state<{ id: string; name: string } | null>(null);

	// Pull in the on-demand modules the moment they're first needed; after that
	// the chunk is cached and the component stays mounted (controlled by `open`).
	$effect(() => {
		if (viewerOpen) void fileViewer.load();
		if (wopiOpen) void wopiEditor.load();
	});
	// Editability of the current context-menu target file, resolved async.
	let ctxCanEditWopi = $state(false);

	// Probe WOPI editability whenever the context target changes to a file. Image
	// files use the inline viewer, so they never show the editor entries.
	$effect(() => {
		const tg = ctxTarget;
		ctxCanEditWopi = false;
		if (!tg || tg.kind !== 'file') return;
		const f = listing.files.find((x) => x.id === tg.id);
		if (!f || (f.mime_type ?? '').startsWith('image/')) return;
		void canEditWithWopi(tg.name).then((ok) => {
			// Guard against a stale resolve after the menu moved to another target.
			if (ctxTarget?.id === tg.id) ctxCanEditWopi = ok;
		});
	});

	function openWopi(id: string, name: string, action: 'edit' | 'view') {
		wopiFile = { id, name };
		wopiAction = action;
		wopiOpen = true;
	}

	async function openWopiTab(id: string, name: string) {
		// New-tab editing posts the token to /wopi/edit; open a blank tab first so
		// the editor URL fetch (async) isn't treated as a popup by the browser.
		const win = window.open('', '_blank');
		try {
			const data = await getEditorUrlWithFallback(id, name, 'edit');
			const url = `/wopi/edit/${encodeURIComponent(id)}?access_token=${encodeURIComponent(data.access_token)}`;
			if (win) win.location.href = url;
			else window.open(url, '_blank');
		} catch (e) {
			win?.close();
			errorToast(e);
		}
	}

	// ── Add audio file to a playlist ──────────────────────────────────────────
	function isAudio(file: FileItem | undefined): boolean {
		return (file?.mime_type ?? '').startsWith('audio/');
	}

	/**
	 * Prompt for a playlist (by name) and add the file to it; an unknown name
	 * creates a new playlist. Mirrors contextMenus.showPlaylistDialog without the
	 * bespoke modal — kept lightweight via the shared prompt dialog.
	 */
	async function addToPlaylist(file: FileItem) {
		let playlists: Awaited<ReturnType<typeof listPlaylists>>;
		try {
			playlists = await listPlaylists();
		} catch (e) {
			errorToast(e);
			return;
		}
		const existing = playlists.map((p) => p.name).join(', ');
		const name = await promptDialog({
			title: t('music.add_to_playlist', 'Add to playlist'),
			message: existing
				? t(
						'music.pick_or_create',
						{ list: existing },
						'Existing: {{list}}. Type a name to add or create.'
					)
				: t('music.create_playlist_hint', 'Type a playlist name to create one.'),
			placeholder: t('music.playlist_name', 'Playlist name'),
			confirmText: t('common.add', 'Add')
		});
		if (!name) return;
		try {
			const match = playlists.find((p) => p.name.toLowerCase() === name.toLowerCase());
			const playlist = match ?? (await createPlaylist(name));
			await addTracks(playlist.id, [file.id]);
			ui.notify(t('music.added_to_playlist', 'Added to playlist'), 'success');
		} catch (e) {
			errorToast(e);
		}
	}

	// ── Open the parent folder of a file ──────────────────────────────────────
	function openParentFolder(file: FileItem) {
		// The current view already lists files inside their folder; navigate to the
		// file's own folder id (handles deep-link / search contexts where the file's
		// folder differs from the current path).
		goto(`/files/${file.folder_id}`);
	}

	// ── Download a folder as a zip archive ────────────────────────────────────
	function downloadFolderZip(folder: { id: string; name: string }) {
		const a = document.createElement('a');
		a.href = folderZipUrl(folder.id);
		a.download = `${folder.name}.zip`;
		document.body.appendChild(a);
		a.click();
		a.remove();
	}

	// ── Recursive folder upload ──────────────────────────────────────────────
	let folderInput = $state<HTMLInputElement | null>(null);

	/**
	 * Upload files that carry a relative directory path, recreating the folder
	 * tree under the current folder. Shared by the folder picker and folder drops.
	 */
	async function uploadTree(entries: { file: File; relativePath: string }[]) {
		if (entries.length === 0) return;
		uploading = true;
		try {
			// Map each relative directory path to its created folder id; '' = current.
			const dirIds = new Map<string, string | null>([['', currentId]]);

			async function ensureDir(relDir: string): Promise<string | null> {
				if (dirIds.has(relDir)) return dirIds.get(relDir) ?? null;
				const parts = relDir.split('/');
				const name = parts.pop() as string;
				const parentId = await ensureDir(parts.join('/'));
				const created = await createFolder(name, parentId);
				dirIds.set(relDir, created.id);
				return created.id;
			}

			for (const { file, relativePath } of entries) {
				const segs = relativePath.split('/');
				segs.pop(); // drop the filename, keep the directory trail
				const dirId = await ensureDir(segs.join('/'));
				await uploadFile(dirId, file);
			}
			ui.notify(t('files.uploaded', 'Upload complete'), 'success');
			await reload();
		} catch (err) {
			errorToast(err);
		} finally {
			uploading = false;
		}
	}

	async function onUploadFolder(e: Event) {
		const input = e.target as HTMLInputElement;
		const files = input.files ? Array.from(input.files) : [];
		// webkitRelativePath: "chosenDir/sub/.../file.ext" — recreate the whole tree.
		await uploadTree(
			files.map((file) => ({
				file,
				relativePath:
					(file as File & { webkitRelativePath?: string }).webkitRelativePath ?? file.name
			}))
		);
		input.value = '';
	}

	const isEmpty = $derived(listing.folders.length === 0 && listing.files.length === 0);
	const viewClass = $derived(
		filesStore.viewMode === 'grid' ? 'files-grid-view' : 'files-list-view'
	);

	// Client-side sort (flat, Drive-style). The listing endpoint returns the
	// folder contents unsorted; sorting here avoids a refetch per column click.
	type SortField = 'name' | 'type' | 'size' | 'modified_at' | 'created_at';
	let sortField = $state<SortField>('name');
	let sortDir = $state<1 | -1>(1);

	function toggleSort(field: SortField) {
		if (sortField === field) sortDir = (sortDir * -1) as 1 | -1;
		else {
			sortField = field;
			sortDir = 1;
		}
	}

	function cmpFolders(a: FolderItem, b: FolderItem): number {
		let v: number;
		if (sortField === 'modified_at') v = a.modified_at - b.modified_at;
		else if (sortField === 'created_at') v = a.created_at - b.created_at;
		// Folders have no size; fall back to name for size/type so they stay stable.
		else v = a.name.localeCompare(b.name);
		return v * sortDir;
	}
	function cmpFiles(a: FileItem, b: FileItem): number {
		let v: number;
		if (sortField === 'size') v = (a.size ?? 0) - (b.size ?? 0);
		else if (sortField === 'modified_at') v = a.modified_at - b.modified_at;
		else if (sortField === 'created_at') v = a.created_at - b.created_at;
		else if (sortField === 'type') v = (a.category ?? '').localeCompare(b.category ?? '');
		else v = a.name.localeCompare(b.name);
		return v * sortDir;
	}

	const sortedFolders = $derived([...listing.folders].sort(cmpFolders));
	const sortedFiles = $derived([...listing.files].sort(cmpFiles));

	/** Flat id order matching how rows are displayed (folders then files). */
	const orderedIds = $derived([...sortedFolders.map((f) => f.id), ...sortedFiles.map((f) => f.id)]);

	// Folders-then-files as one ordered, discriminated list so the (flat) view can
	// be windowed by a single VirtualList. Content width drives the grid columns.
	type Entry = { kind: 'folder'; folder: FolderItem } | { kind: 'file'; file: FileItem };
	const entries = $derived<Entry[]>([
		...sortedFolders.map((folder) => ({ kind: 'folder' as const, folder })),
		...sortedFiles.map((file) => ({ kind: 'file' as const, file }))
	]);
	const entryKey = (e: Entry): string => (e.kind === 'folder' ? e.folder.id : e.file.id);
	let gridWidth = $state(0);

	// ── Group-by / swimlanes ─────────────────────────────────────────────────
	// Mirrors GROUP_BY_DEFS in static/js/app/filesView.js: a flat list ('') plus
	// Type / Size / Modified date / Created date dimensions. Folders always group
	// into their own lane (Folder / "Folders" size sentinel) ahead of the files.
	type GroupBy = '' | 'type' | 'size' | 'modifiedAt' | 'createdAt';
	let groupBy = $state<GroupBy>('');

	interface ResourceGroup {
		key: string;
		label: string;
		folders: FolderItem[];
		files: FileItem[];
	}

	function folderGroupKey(folder: FolderItem): string {
		if (groupBy === 'type') return t('files.file_types.folder', 'Folders');
		if (groupBy === 'size') return sizeBucket(-1);
		if (groupBy === 'modifiedAt') return dateBucket(folder.modified_at);
		if (groupBy === 'createdAt') return dateBucket(folder.created_at);
		return '';
	}
	function fileGroupKey(file: FileItem): string {
		if (groupBy === 'type') return typeLabel(file.category);
		if (groupBy === 'size') return sizeBucket(file.size ?? 0);
		if (groupBy === 'modifiedAt') return dateBucket(file.modified_at);
		if (groupBy === 'createdAt') return dateBucket(file.created_at);
		return '';
	}

	// Grouped rendering: ordered lanes preserving the sorted folder-then-file order
	// within each lane. Lanes appear in first-seen order (folders precede files).
	const groups = $derived.by<ResourceGroup[]>(() => {
		if (groupBy === '') return [];
		const map = new Map<string, ResourceGroup>();
		const ensure = (key: string): ResourceGroup => {
			let g = map.get(key);
			if (!g) {
				g = { key, label: key, folders: [], files: [] };
				map.set(key, g);
			}
			return g;
		};
		for (const folder of sortedFolders) ensure(folderGroupKey(folder)).folders.push(folder);
		for (const file of sortedFiles) ensure(fileGroupKey(file)).files.push(file);
		return [...map.values()];
	});

	// ── Toolbar controls (upload split-button + group-by popup menu) ─────────
	// The group-by popup + sort-direction + view toggle live in the shared
	// <ListToolbar>; this page only owns the upload split-button dropdown.
	let uploadMenuOpen = $state(false);

	interface GroupByDef {
		key: GroupBy;
		label: string;
		icon: string;
		/** Sort field implied by this dimension (the old group-by drove order_by). */
		sort?: SortField;
	}
	// Mirrors the old GROUP_BY_DEFS: "Name" is the default (flat, sorted by name)
	// entry — there is no "None" option — followed by the swimlane dimensions.
	const GROUP_BYS = $derived<GroupByDef[]>([
		{ key: '', label: t('files.name', 'Name'), icon: 'arrow-up-a-z', sort: 'name' },
		{ key: 'type', label: t('groupby.type', 'Type'), icon: 'layer-group', sort: 'type' },
		{ key: 'size', label: t('groupby.size', 'Size'), icon: 'layer-group', sort: 'size' },
		{
			key: 'modifiedAt',
			label: t('groupby.modifiedAt', 'Modified date'),
			icon: 'layer-group',
			sort: 'modified_at'
		},
		{
			key: 'createdAt',
			label: t('groupby.createdAt', 'Created date'),
			icon: 'layer-group',
			sort: 'created_at'
		}
	]);

	/** Group-by chosen in the toolbar — also sets the matching sort field. */
	function onPickGroup(key: string) {
		groupBy = key as GroupBy;
		const def = GROUP_BYS.find((g) => g.key === key);
		if (def?.sort) sortField = def.sort;
	}

	// Close the upload popup when clicking outside of it.
	$effect(() => {
		if (!uploadMenuOpen) return;
		const onDown = (e: MouseEvent) => {
			if (!(e.target as HTMLElement).closest('.upload-dropdown')) uploadMenuOpen = false;
		};
		window.addEventListener('pointerdown', onDown);
		return () => window.removeEventListener('pointerdown', onDown);
	});

	const SKELETON = [0, 1, 2, 3, 4, 5, 6, 7];

	// Reload whenever the route path changes.
	$effect(() => {
		// reference pathSegments so the effect re-runs on navigation
		void pathSegments;
		void load();
	});

	// The command palette's "Upload files" action navigates here then dispatches
	// this event so the hidden file picker opens (the input lives on this page).
	$effect(() => {
		const open = () => fileInput?.click();
		window.addEventListener('oxicloud:upload-files', open);
		return () => window.removeEventListener('oxicloud:upload-files', open);
	});
</script>

<svelte:head><title>{t('nav.files', 'Files')} · OxiCloud</title></svelte:head>

<svelte:window onkeydown={onKeydown} />

<div
	class="files-page"
	class:dropzone-active={dragOver}
	role="region"
	aria-label={t('nav.files', 'Files')}
	ondragover={(e) => {
		e.preventDefault();
		dragOver = true;
	}}
	ondragleave={() => (dragOver = false)}
	ondrop={onDrop}
>
	<div class="page-sticky-header">
		<!-- Hidden upload inputs stay mounted even while the batch bar is shown. -->
		<input bind:this={fileInput} type="file" multiple hidden onchange={onUpload} />
		<input
			bind:this={folderInput}
			type="file"
			multiple
			hidden
			webkitdirectory
			onchange={onUploadFolder}
		/>

		<ListToolbar
			groups={GROUP_BYS}
			{groupBy}
			reversed={sortDir === -1}
			ongroup={onPickGroup}
			ondirection={() => (sortDir = (sortDir * -1) as 1 | -1)}
		>
			{#snippet start()}
				{#if selectedCount > 0}
					<div class="action-buttons batch-selection-bar">
						<div class="list-header-checkbox">
							<button
								class="batch-bar-close"
								title={t('files.cancel_selection', 'Cancel selection')}
								aria-label={t('files.cancel_selection', 'Cancel selection')}
								onclick={clearSelection}
							>
								<Icon name="times" />
							</button>
							<span class="batch-bar-count"
								>{t('files.selected_count', { n: selectedCount }, '{{n}} selected')}</span
							>
						</div>
						<div class="batch-selection-info">
							<div class="batch-bar-actions">
								<button
									class="batch-btn"
									title={t('files.add_favorites', 'Add to favorites')}
									onclick={() => void batchFavorites()}
								>
									<Icon name="star" />
									<span>{t('files.add_favorites', 'Add to favorites')}</span>
								</button>
								<button class="batch-btn" title={t('files.move', 'Move')} onclick={batchMove}>
									<Icon name="arrows-alt" />
									<span>{t('files.move', 'Move')}</span>
								</button>
								<button class="batch-btn" title={t('files.copy', 'Copy')} onclick={batchCopy}>
									<Icon name="copy" />
									<span>{t('files.copy', 'Copy')}</span>
								</button>
								<button
									class="batch-btn"
									title={t('common.download', 'Download')}
									onclick={() => void batchDownload()}
								>
									<Icon name="download" />
									<span>{t('common.download', 'Download')}</span>
								</button>
								<button
									class="batch-btn batch-btn-danger"
									title={t('common.delete', 'Delete')}
									onclick={batchDelete}
								>
									<Icon name="trash" />
									<span>{t('common.delete', 'Delete')}</span>
								</button>
							</div>
						</div>
					</div>
				{:else}
					<div class="action-buttons">
						<div class="upload-dropdown">
							<button
								class="btn btn-primary"
								onclick={() => (uploadMenuOpen = !uploadMenuOpen)}
								disabled={uploading}
								aria-haspopup="true"
								aria-expanded={uploadMenuOpen}
							>
								<Icon name="cloud-upload-alt" class="icon-mr" />
								<span
									>{uploading
										? t('files.uploading', 'Uploading…')
										: t('actions.upload', 'Upload')}</span
								>
								<Icon name="caret-down" class="upload-caret" />
							</button>
							{#if uploadMenuOpen}
								<div class="upload-dropdown-menu">
									<button
										class="upload-dropdown-item"
										onclick={() => {
											uploadMenuOpen = false;
											fileInput?.click();
										}}
									>
										<Icon name="file" />
										<span>{t('actions.upload_files', 'Upload files')}</span>
									</button>
									<button
										class="upload-dropdown-item"
										onclick={() => {
											uploadMenuOpen = false;
											folderInput?.click();
										}}
									>
										<Icon name="folder-open" />
										<span>{t('actions.upload_folder', 'Upload folder')}</span>
									</button>
								</div>
							{/if}
						</div>
						<button class="btn btn-secondary" onclick={onNewFolder}>
							<Icon name="folder-plus" class="icon-mr" />
							<span>{t('actions.new_folder', 'New folder')}</span>
						</button>
					</div>
				{/if}
			{/snippet}
		</ListToolbar>

		<nav class="breadcrumb" aria-label="Breadcrumb">
			<a
				href="/files"
				class="breadcrumb-item breadcrumb-home breadcrumb-link"
				title={t('breadcrumb.home', 'Home')}
				ondragover={(e) => e.dataTransfer?.types.includes(DRAG_TYPE) && e.preventDefault()}
				ondrop={(e) => session.homeFolderId && onCrumbDrop(e, session.homeFolderId)}
			>
				<Icon name="home" />
			</a>
			{#each crumbs as c, i (c.id)}
				<span class="breadcrumb-separator">&gt;</span>
				{#if i === crumbs.length - 1}
					<span class="breadcrumb-item breadcrumb-current">{c.name}</span>
				{:else}
					<a
						href={crumbHref(i)}
						class="breadcrumb-item breadcrumb-link"
						ondragover={(e) => e.dataTransfer?.types.includes(DRAG_TYPE) && e.preventDefault()}
						ondrop={(e) => onCrumbDrop(e, c.id)}>{c.name}</a
					>
				{/if}
			{/each}
		</nav>
	</div>

	{#if error}
		<EmptyState title={error} error />
	{:else if showSkeleton && isEmpty}
		<SkeletonList count={SKELETON.length} />
	{:else if isEmpty}
		<EmptyState
			title={t('files.empty_title', 'This folder is empty')}
			hint={t('files.empty_hint', 'Drop files here or use the Upload button to add files.')}
		/>
	{:else}
		<div class="files-container" bind:clientWidth={gridWidth}>
			{#if groupBy !== ''}
				<div class={viewClass}>
					{@render fileListHeader()}
					{#each groups as group (group.key)}
						<div class="resource-list__swimlane-header">{group.label}</div>
						{#each group.folders as folder (folder.id)}
							{@render folderRow(folder)}
						{/each}
						{#each group.files as file (file.id)}
							{@render fileRow(file)}
						{/each}
					{/each}
				</div>
			{:else if filesStore.viewMode === 'list'}
				<!-- Flat list: only the rows near the viewport are mounted. -->
				<div class="files-list-view">
					{@render fileListHeader()}
					<VirtualList items={entries} rowHeight={56} key={entryKey} row={entryRow} />
				</div>
			{:else}
				<!-- Grid: the windowed list's inner element IS the card grid. -->
				<VirtualList
					items={entries}
					columns={gridColumns(gridWidth)}
					rowHeight={240}
					windowClass="files-grid-view"
					key={entryKey}
					row={entryRow}
				/>
			{/if}
		</div>
	{/if}
</div>

{#snippet fileListHeader()}
	<div class="list-header">
		<div class="list-header-checkbox">
			<input
				type="checkbox"
				aria-label={t('files.select_all', 'Select all')}
				checked={selectedCount > 0 && selectedCount === totalCount}
				indeterminate={selectedCount > 0 && selectedCount < totalCount}
				onchange={toggleSelectAll}
			/>
		</div>
		{#each [{ f: 'name', l: t('files.col_name', 'Name') }, { f: 'owner', l: t('files.col_owner', 'Owner') }, { f: 'type', l: t('files.col_type', 'Type') }, { f: 'size', l: t('files.col_size', 'Size') }, { f: 'modified_at', l: t('files.col_modified', 'Modified') }] as col (col.f)}
			{#if col.f === 'owner'}
				<div class="list-header-owner">{col.l}</div>
			{:else}
				<button
					class="list-header-sort"
					class:is-active={sortField === col.f}
					data-sort-field={col.f}
					onclick={() => toggleSort(col.f as SortField)}
				>
					{col.l}
					{#if sortField === col.f}
						<Icon
							name={sortDir === 1 ? 'arrow-down' : 'arrow-up'}
							class="list-header-sort__arrow"
						/>
					{/if}
				</button>
			{/if}
		{/each}
		<div></div>
	</div>
{/snippet}

{#snippet entryRow(e: Entry)}
	{#if e.kind === 'folder'}
		{@render folderRow(e.folder)}
	{:else}
		{@render fileRow(e.file)}
	{/if}
{/snippet}

{#snippet folderRow(folder: FolderItem)}
	<div
		class="file-item"
		class:selected={selected.has(folder.id)}
		class:drop-target={dropFolderId === folder.id}
		role="button"
		tabindex="0"
		draggable="true"
		ondragstart={(e) => onItemDragStart(e, 'folder', folder.id, folder.name)}
		ondragover={(e) => {
			if (e.dataTransfer?.types.includes(DRAG_TYPE)) {
				e.preventDefault();
				dropFolderId = folder.id;
			}
		}}
		ondragleave={() => {
			if (dropFolderId === folder.id) dropFolderId = null;
		}}
		ondrop={(e) => onFolderDrop(e, folder)}
		ondblclick={() => openFolder(folder)}
		onclick={(e) => {
			if (!handleSelectionClick(e, folder.id)) openFolder(folder);
		}}
		oncontextmenu={(e) => openContext(e, 'folder', folder.id, folder.name)}
		onkeydown={(e) => e.key === 'Enter' && openFolder(folder)}
	>
		<div class="checkbox-cell">
			<input
				type="checkbox"
				checked={selected.has(folder.id)}
				aria-label={folder.name}
				onclick={(e) => {
					e.stopPropagation();
					toggleSelected(folder.id);
				}}
			/>
		</div>
		<div class="name-cell">
			<div class="file-icon"><Icon name="folder" /></div>
			<span title={folder.name}>{folder.name}</span>
			{#if favoriteIds.has(folder.id)}<div
					class="item-badge item-badge--fav"
					title={t('files.favorited', 'Favorite')}
				>
					<Icon name="star" />
				</div>{/if}
			{#if sharedIds.has(folder.id)}<div
					class="file-badge file-badge-shared"
					title={t('files.shared', 'Shared')}
				>
					<Icon name="oxiexport" />
				</div>{/if}
		</div>
		<div class="grid-meta">
			<span class="grid-meta__date">{relativeTimeAgo(folder.modified_at)}</span>
		</div>
		<div class="owner-cell">{ownerLabel(folder.owner_id, session.user?.id ?? null)}</div>
		<div class="type-cell">{t('files.file_types.folder', 'Folder')}</div>
		<div class="size-cell">—</div>
		<div class="date-cell">{formatDate(folder.modified_at)}</div>
		<div class="action-cell">
			<button
				class="favorite-star"
				class:active={favoriteIds.has(folder.id)}
				title={favoriteIds.has(folder.id)
					? t('files.unfavorite', 'Remove favorite')
					: t('files.favorite', 'Add favorite')}
				aria-pressed={favoriteIds.has(folder.id)}
				onclick={(e) => {
					e.stopPropagation();
					void toggleFavorite('folder', folder.id);
				}}><Icon name={favoriteIds.has(folder.id) ? 'star' : 'star-outline'} /></button
			>
			<button
				class="btn-action"
				title={t('files.share', 'Share')}
				onclick={(e) => {
					e.stopPropagation();
					openShare('folder', folder.id, folder.name);
				}}><Icon name="link" /></button
			>
			<button
				class="btn-action"
				title={t('files.move', 'Move')}
				onclick={(e) => {
					e.stopPropagation();
					openMove('folder', folder.id, folder.name);
				}}><Icon name="arrows-alt" /></button
			>
			<button
				class="btn-action"
				title={t('common.rename', 'Rename')}
				onclick={(e) => {
					e.stopPropagation();
					renameItem('folder', folder.id, folder.name);
				}}><Icon name="pen" /></button
			>
			<button
				class="btn-action btn-action--delete"
				title={t('common.delete', 'Delete')}
				onclick={(e) => {
					e.stopPropagation();
					deleteItem('folder', folder.id, folder.name);
				}}><Icon name="trash" /></button
			>
			<button
				class="file-actions"
				title={t('files.more_actions', 'More actions')}
				aria-label={t('files.more_actions', 'More actions')}
				aria-haspopup="menu"
				onclick={(e) => openContext(e, 'folder', folder.id, folder.name)}
				><Icon name="ellipsis-v" /></button
			>
		</div>
	</div>
{/snippet}

{#snippet fileRow(file: FileItem)}
	<div
		class="file-item"
		class:selected={selected.has(file.id)}
		role="button"
		tabindex="0"
		draggable="true"
		ondragstart={(e) => onItemDragStart(e, 'file', file.id, file.name)}
		ondblclick={() => openFile(file)}
		onclick={(e) => {
			if (!handleSelectionClick(e, file.id)) openFile(file);
		}}
		oncontextmenu={(e) => openContext(e, 'file', file.id, file.name)}
		onkeydown={(e) => e.key === 'Enter' && openFile(file)}
	>
		<div class="checkbox-cell">
			<input
				type="checkbox"
				checked={selected.has(file.id)}
				aria-label={file.name}
				onclick={(e) => {
					e.stopPropagation();
					toggleSelected(file.id);
				}}
			/>
		</div>
		<div class="name-cell">
			<div class="file-icon">
				{#if canThumbnail(file)}
					<img
						class="file-thumb"
						src={fileThumbnailUrl(file.id)}
						alt=""
						loading="lazy"
						onerror={(e) => ((e.currentTarget as HTMLImageElement).style.display = 'none')}
					/>
				{:else}
					<Icon name={iconNameFromClass(file.icon_class)} />
				{/if}
			</div>
			<span title={file.name}>{file.name}</span>
			{#if favoriteIds.has(file.id)}<div
					class="item-badge item-badge--fav"
					title={t('files.favorited', 'Favorite')}
				>
					<Icon name="star" />
				</div>{/if}
			{#if sharedIds.has(file.id)}<div
					class="file-badge file-badge-shared"
					title={t('files.shared', 'Shared')}
				>
					<Icon name="oxiexport" />
				</div>{/if}
		</div>
		<div class="grid-meta">
			<span class="grid-meta__date">{relativeTimeAgo(file.modified_at)}</span>
			{#if file.size != null}<span class="grid-meta__size">{formatBytes(file.size)}</span>{/if}
		</div>
		<div class="owner-cell">{ownerLabel(file.owner_id, session.user?.id ?? null)}</div>
		<div class="type-cell">{typeLabel(file.category)}</div>
		<div class="size-cell">{file.size != null ? formatBytes(file.size) : ''}</div>
		<div class="date-cell">{formatDate(file.modified_at)}</div>
		<div class="action-cell">
			<button
				class="favorite-star"
				class:active={favoriteIds.has(file.id)}
				title={favoriteIds.has(file.id)
					? t('files.unfavorite', 'Remove favorite')
					: t('files.favorite', 'Add favorite')}
				aria-pressed={favoriteIds.has(file.id)}
				onclick={(e) => {
					e.stopPropagation();
					void toggleFavorite('file', file.id);
				}}><Icon name={favoriteIds.has(file.id) ? 'star' : 'star-outline'} /></button
			>
			<button
				class="btn-action"
				title={t('files.share', 'Share')}
				onclick={(e) => {
					e.stopPropagation();
					openShare('file', file.id, file.name);
				}}><Icon name="link" /></button
			>
			<button
				class="btn-action"
				title={t('files.move', 'Move')}
				onclick={(e) => {
					e.stopPropagation();
					openMove('file', file.id, file.name);
				}}><Icon name="arrows-alt" /></button
			>
			<a
				class="btn-action"
				href={fileDownloadUrl(file.id)}
				download
				title={t('common.download', 'Download')}
				onclick={(e) => e.stopPropagation()}><Icon name="download" /></a
			>
			<button
				class="btn-action"
				title={t('common.rename', 'Rename')}
				onclick={(e) => {
					e.stopPropagation();
					renameItem('file', file.id, file.name);
				}}><Icon name="pen" /></button
			>
			<button
				class="btn-action btn-action--delete"
				title={t('common.delete', 'Delete')}
				onclick={(e) => {
					e.stopPropagation();
					deleteItem('file', file.id, file.name);
				}}><Icon name="trash" /></button
			>
			<button
				class="file-actions"
				title={t('files.more_actions', 'More actions')}
				aria-label={t('files.more_actions', 'More actions')}
				aria-haspopup="menu"
				onclick={(e) => openContext(e, 'file', file.id, file.name)}
				><Icon name="ellipsis-v" /></button
			>
		</div>
	</div>
{/snippet}

<MoveDialog
	bind:open={moveOpen}
	item={actionTarget}
	items={moveItems}
	mode={moveMode}
	onmoved={() => {
		clearSelection();
		void reload();
	}}
/>
<ShareDialog
	bind:open={shareOpen}
	item={actionTarget}
	onshared={(id) => (sharedIds = new Set(sharedIds).add(id))}
/>
{#if fileViewer.component}
	{@const FileViewer = fileViewer.component}
	<FileViewer bind:open={viewerOpen} file={viewerFile} />
{/if}
{#if wopiEditor.component}
	{@const WopiEditor = wopiEditor.component}
	<WopiEditor
		bind:open={wopiOpen}
		fileId={wopiFile?.id ?? null}
		fileName={wopiFile?.name ?? ''}
		action={wopiAction}
	/>
{/if}

{#if ctxOpen && ctxTarget}
	<div
		class="ctx-scrim"
		role="presentation"
		onclick={closeContext}
		oncontextmenu={(e) => e.preventDefault()}
	></div>
	<div class="ctx-menu" style:left="{ctxX}px" style:top="{ctxY}px" role="menu">
		{#if ctxTarget.kind === 'folder'}
			<button
				class="ctx-item"
				role="menuitem"
				onclick={() => {
					const id = ctxTarget!.id;
					closeContext();
					goto(`/files/${[...pathSegments, id].join('/')}`);
				}}><Icon name="folder-open" /> {t('files.open', 'Open')}</button
			>
			<button
				class="ctx-item"
				role="menuitem"
				onclick={() => {
					const tg = ctxTarget!;
					closeContext();
					downloadFolderZip({ id: tg.id, name: tg.name });
				}}><Icon name="download" /> {t('files.download_zip', 'Download as ZIP')}</button
			>
		{:else}
			<button
				class="ctx-item"
				role="menuitem"
				onclick={() => {
					const f = listing.files.find((x) => x.id === ctxTarget!.id);
					closeContext();
					if (f) openFile(f);
				}}><Icon name="eye" /> {t('files.open', 'Open')}</button
			>
			{#if ctxCanEditWopi}
				<button
					class="ctx-item"
					role="menuitem"
					onclick={() => {
						const tg = ctxTarget!;
						closeContext();
						openWopi(tg.id, tg.name, 'edit');
					}}><Icon name="pen" /> {t('files.edit', 'Edit')}</button
				>
				<button
					class="ctx-item"
					role="menuitem"
					onclick={() => {
						const tg = ctxTarget!;
						closeContext();
						void openWopiTab(tg.id, tg.name);
					}}><Icon name="external-link-alt" /> {t('files.edit_new_tab', 'Edit in new tab')}</button
				>
			{/if}
			<a
				class="ctx-item"
				role="menuitem"
				href={fileDownloadUrl(ctxTarget.id)}
				download
				onclick={closeContext}><Icon name="download" /> {t('common.download', 'Download')}</a
			>
			<button
				class="ctx-item"
				role="menuitem"
				onclick={() => {
					const f = listing.files.find((x) => x.id === ctxTarget!.id);
					closeContext();
					if (f) openParentFolder(f);
				}}><Icon name="folder-open" /> {t('files.open_parent', 'Open parent folder')}</button
			>
			{#if isAudio(listing.files.find((x) => x.id === ctxTarget!.id))}
				<button
					class="ctx-item"
					role="menuitem"
					onclick={() => {
						const f = listing.files.find((x) => x.id === ctxTarget!.id);
						closeContext();
						if (f) void addToPlaylist(f);
					}}><Icon name="music" /> {t('music.add_to_playlist', 'Add to playlist')}</button
				>
			{/if}
		{/if}
		<button
			class="ctx-item"
			role="menuitem"
			onclick={() => {
				const tg = ctxTarget!;
				closeContext();
				openShare(tg.kind, tg.id, tg.name);
			}}><Icon name="link" /> {t('files.share', 'Share')}</button
		>
		<button
			class="ctx-item"
			role="menuitem"
			onclick={() => {
				const tg = ctxTarget!;
				closeContext();
				openMove(tg.kind, tg.id, tg.name);
			}}><Icon name="arrows-alt" /> {t('files.move', 'Move')}</button
		>
		<button
			class="ctx-item"
			role="menuitem"
			onclick={() => {
				const tg = ctxTarget!;
				closeContext();
				openCopy(tg.kind, tg.id, tg.name);
			}}><Icon name="copy" /> {t('files.copy', 'Copy')}</button
		>
		<button
			class="ctx-item"
			role="menuitem"
			onclick={() => {
				const tg = ctxTarget!;
				closeContext();
				void toggleFavorite(tg.kind, tg.id);
			}}
		>
			<Icon name="star" />
			{favoriteIds.has(ctxTarget.id)
				? t('files.unfavorite', 'Remove favorite')
				: t('files.favorite', 'Add favorite')}
		</button>
		<button
			class="ctx-item"
			role="menuitem"
			onclick={() => {
				const tg = ctxTarget!;
				closeContext();
				void renameItem(tg.kind, tg.id, tg.name);
			}}><Icon name="pen" /> {t('common.rename', 'Rename')}</button
		>
		<button
			class="ctx-item ctx-item--danger"
			role="menuitem"
			onclick={() => {
				const tg = ctxTarget!;
				closeContext();
				void deleteItem(tg.kind, tg.id, tg.name);
			}}><Icon name="trash" /> {t('common.delete', 'Delete')}</button
		>
	</div>
{/if}

<style>
	.files-page {
		min-height: 100%;
	}

	.files-page.dropzone-active {
		outline: 2px dashed var(--color-accent);
		outline-offset: -8px;
		border-radius: var(--radius-xl);
	}

	.page-sticky-header {
		display: flex;
		flex-direction: column;
		gap: var(--space-3);
	}

	.action-cell {
		display: flex;
		gap: var(--space-1);
		justify-content: flex-end;
	}

	.btn-action--delete:hover {
		color: var(--color-danger-text);
	}

	.btn-action {
		text-decoration: none;
	}

	/* Owner column header — non-sortable, so it's a plain div rather than a
	   sort button. Inherits the header row's weight/colour. */
	.list-header-owner {
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.item-badge {
		display: inline-flex;
		align-items: center;
		margin-left: var(--space-1);
		font-size: 0.75rem;
		color: var(--color-text-muted);
	}

	.item-badge--fav {
		color: var(--color-warning-text, var(--color-accent));
	}

	.ctx-scrim {
		position: fixed;
		inset: 0;
		z-index: 90;
	}

	.ctx-menu {
		position: fixed;
		z-index: 100;
		min-width: 12rem;
		padding: var(--space-1);
		background: var(--color-bg-surface);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-md);
		box-shadow: var(--shadow-lg, 0 10px 30px var(--color-overlay-shadow));
	}

	.ctx-item {
		display: flex;
		align-items: center;
		gap: 0.6rem;
		width: 100%;
		padding: 0.5rem 0.6rem;
		border: none;
		background: none;
		color: var(--color-text);
		cursor: pointer;
		text-align: left;
		text-decoration: none;
		border-radius: var(--radius-sm);
		font-size: var(--text-sm);
	}

	.ctx-item:hover {
		background: var(--color-bg-hover);
	}

	.ctx-item--danger {
		color: var(--color-danger-text);
	}
</style>
