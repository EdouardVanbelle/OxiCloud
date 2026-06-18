<script lang="ts">
	import { errorMessage, errorToast } from '$lib/utils/errors';
	import {
		clearPluginLogs,
		createUser,
		deletePlugin,
		deleteUser,
		getDashboard,
		getMigration,
		getOidcSettings,
		getPluginLogs,
		getPluginRetention,
		getSmtpInfo,
		getStorageSettings,
		installPlugin,
		listPlugins,
		listUsers,
		migrationAction,
		resetUserPassword,
		saveOidc,
		savePluginRetention,
		saveStorage,
		sendSmtpTest,
		setPluginEnabled,
		setRegistrationEnabled,
		setUserActive,
		setUserQuota,
		setUserRole,
		testOidc,
		testStorage,
		verifyMigration,
		type AdminDashboard,
		type MigrationStatus,
		type MigrationVerifyResult,
		type OidcSettings,
		type OidcTestResult,
		type PluginInfo,
		type PluginLogEntry,
		type PluginRetention,
		type SmtpInfo,
		type SmtpTestResult,
		type StorageSettings,
		type StorageTestResult
	} from '$lib/api/endpoints/admin';
	import type { User } from '$lib/api/types';
	import Icon from '$lib/icons/Icon.svelte';
	import Modal from '$lib/components/Modal.svelte';
	import { t } from '$lib/i18n/index.svelte';
	import { session } from '$lib/stores/session.svelte';
	import { ui } from '$lib/stores/ui.svelte';
	import { formatBytes } from '$lib/utils/format';

	const PAGE_SIZE = 25;
	const LOGS_PAGE_SIZE = 50;

	/** The signed-in admin's id — used to disable destructive actions on self. */
	const currentAdminId = $derived(session.user?.id ?? '');

	/**
	 * Render an ISO timestamp as a coarse relative time ("3 min ago"). Ported
	 * from formatRelativeTime in static/js/core/formatters.js; an empty/missing
	 * value reads as "Never" (matching the OLD admin user table).
	 */
	function timeAgo(dateStr?: string | null): string {
		if (!dateStr) return t('admin.never', 'Never');
		const then = new Date(dateStr).getTime();
		if (!Number.isFinite(then)) return t('admin.never', 'Never');
		const secs = Math.round((Date.now() - then) / 1000);
		if (secs < 60) return t('admin.time_just_now', 'just now');
		const mins = Math.round(secs / 60);
		if (mins < 60) return t('admin.time_min_ago', { n: mins }, '{{n}} min ago');
		const hours = Math.round(mins / 60);
		if (hours < 24) return t('admin.time_hour_ago', { n: hours }, '{{n}} h ago');
		const days = Math.round(hours / 24);
		if (days < 30) return t('admin.time_day_ago', { n: days }, '{{n}} d ago');
		return new Date(dateStr).toLocaleDateString();
	}

	/** Quota unit options (bytes per unit) for the quota/create modals. */
	const QUOTA_UNITS = [
		{ value: 1024 ** 2, label: 'MB' },
		{ value: 1024 ** 3, label: 'GB' },
		{ value: 1024 ** 4, label: 'TB' }
	] as const;

	/* ── Styled confirm modal (replaces native confirm) ── */
	let confirmState = $state<{ message: string; resolve: (ok: boolean) => void } | null>(null);
	function showConfirm(message: string): Promise<boolean> {
		return new Promise((resolve) => {
			confirmState = { message, resolve };
		});
	}
	function resolveConfirm(ok: boolean) {
		confirmState?.resolve(ok);
		confirmState = null;
	}

	type Tab = 'dashboard' | 'users' | 'plugins' | 'oidc' | 'storage' | 'smtp';
	let tab = $state<Tab>('dashboard');

	// Dashboard
	let dashboard = $state<AdminDashboard | null>(null);
	let dashboardError = $state<string | null>(null);

	// SMTP
	let smtp = $state<SmtpInfo | null>(null);
	let smtpTo = $state('');
	let smtpResult = $state<SmtpTestResult | null>(null);
	let smtpSending = $state(false);

	async function loadDashboard() {
		dashboardError = null;
		try {
			dashboard = await getDashboard();
		} catch (e) {
			dashboardError = errorMessage(e);
		}
	}

	async function toggleRegistration(enabled: boolean) {
		try {
			await setRegistrationEnabled(enabled);
			if (dashboard) dashboard.registration_enabled = enabled;
		} catch (e) {
			reportError(e);
			await loadDashboard();
		}
	}

	async function loadSmtp() {
		try {
			smtp = await getSmtpInfo();
		} catch (e) {
			reportError(e);
		}
	}

	async function runSmtpTest() {
		if (!smtpTo.trim()) return;
		smtpSending = true;
		smtpResult = null;
		try {
			smtpResult = await sendSmtpTest(smtpTo.trim());
		} catch (e) {
			smtpResult = { success: false, message: errorMessage(e) };
		} finally {
			smtpSending = false;
		}
	}

	// OIDC
	let oidc = $state<(OidcSettings & { client_secret?: string }) | null>(null);
	let oidcTest = $state<OidcTestResult | null>(null);
	let oidcMsg = $state<{ text: string; ok: boolean } | null>(null);
	let oidcSaving = $state(false);

	async function loadOidc() {
		try {
			oidc = await getOidcSettings();
		} catch (e) {
			oidcMsg = { text: errorMessage(e), ok: false };
		}
	}
	async function runOidcTest() {
		if (!oidc?.issuer_url) return;
		oidcTest = await testOidc(oidc.issuer_url);
		if (oidcTest.success && oidcTest.provider_name_suggestion && !oidc.provider_name) {
			oidc.provider_name = oidcTest.provider_name_suggestion;
		}
	}
	async function doSaveOidc() {
		if (!oidc) return;
		oidcSaving = true;
		oidcMsg = null;
		try {
			await saveOidc({
				enabled: oidc.enabled,
				issuer_url: oidc.issuer_url.trim(),
				client_id: oidc.client_id.trim(),
				client_secret: oidc.client_secret || null,
				scopes: oidc.scopes || null,
				auto_provision: oidc.auto_provision,
				admin_groups: oidc.admin_groups || null,
				disable_password_login: oidc.disable_password_login,
				provider_name: oidc.provider_name || null
			});
			oidcMsg = { text: t('admin.settings_saved_ok', 'Settings saved.'), ok: true };
		} catch (e) {
			oidcMsg = { text: errorMessage(e), ok: false };
		} finally {
			oidcSaving = false;
		}
	}

	// Storage
	const STORAGE_PRESETS: Record<string, { endpoint: string; region: string; pathStyle: boolean }> =
		{
			custom: { endpoint: '', region: '', pathStyle: false },
			aws: { endpoint: '', region: 'us-east-1', pathStyle: false },
			backblaze: {
				endpoint: 'https://s3.{region}.backblazeb2.com',
				region: 'us-west-004',
				pathStyle: false
			},
			'cloudflare-r2': {
				endpoint: 'https://{accountId}.r2.cloudflarestorage.com',
				region: 'auto',
				pathStyle: true
			},
			minio: { endpoint: 'http://localhost:9000', region: 'us-east-1', pathStyle: true },
			digitalocean: {
				endpoint: 'https://{region}.digitaloceanspaces.com',
				region: 'nyc3',
				pathStyle: false
			},
			wasabi: {
				endpoint: 'https://s3.{region}.wasabisys.com',
				region: 'us-east-1',
				pathStyle: false
			}
		};
	let storage = $state<StorageSettings | null>(null);
	let sForm = $state({
		backend: 'local',
		preset: 'custom',
		endpoint: '',
		bucket: '',
		region: '',
		accessKey: '',
		secretKey: '',
		pathStyle: false
	});
	let storageMsg = $state<{ text: string; ok: boolean } | null>(null);
	let storageBusy = $state(false);

	async function loadStorage() {
		try {
			storage = await getStorageSettings();
			sForm = {
				backend: storage.backend ?? 'local',
				preset: 'custom',
				endpoint: storage.s3_endpoint_url ?? '',
				bucket: storage.s3_bucket ?? '',
				region: storage.s3_region ?? '',
				accessKey: '',
				secretKey: '',
				pathStyle: storage.s3_force_path_style ?? false
			};
		} catch (e) {
			storageMsg = { text: errorMessage(e), ok: false };
		}
	}
	function applyPreset() {
		const p = STORAGE_PRESETS[sForm.preset];
		if (!p) return;
		if (p.endpoint) sForm.endpoint = p.endpoint;
		if (p.region) sForm.region = p.region;
		sForm.pathStyle = p.pathStyle;
	}
	function storageBody() {
		return {
			backend: sForm.backend,
			s3_endpoint_url: sForm.endpoint.trim() || null,
			s3_bucket: sForm.bucket.trim() || null,
			s3_region: sForm.region.trim() || null,
			s3_access_key: sForm.accessKey || null,
			s3_secret_key: sForm.secretKey || null,
			s3_force_path_style: sForm.pathStyle
		};
	}
	async function doSaveStorage() {
		storageBusy = true;
		storageMsg = null;
		try {
			await saveStorage(storageBody());
			storageMsg = { text: t('admin.storage_saved', 'Storage settings saved.'), ok: true };
			await loadStorage();
		} catch (e) {
			storageMsg = { text: errorMessage(e), ok: false };
		} finally {
			storageBusy = false;
		}
	}
	async function doTestStorage() {
		storageBusy = true;
		storageMsg = null;
		try {
			const r: StorageTestResult = await testStorage(storageBody());
			const ok = r.connected ?? r.success ?? false;
			if (ok) {
				let text = t('admin.storage_test_success', 'Connection successful');
				if (r.backend_type) text += ` (${r.backend_type})`;
				if (r.available_bytes != null)
					text += ` — ${formatBytes(r.available_bytes)} ${t('admin.available', 'available')}`;
				storageMsg = { text, ok: true };
			} else {
				storageMsg = {
					text: `${t('admin.storage_test_failure', 'Connection failed')}: ${r.message ?? ''}`,
					ok: false
				};
			}
		} catch (e) {
			storageMsg = { text: errorMessage(e), ok: false };
		} finally {
			storageBusy = false;
		}
	}

	// Migration
	let migration = $state<MigrationStatus | null>(null);
	let migrationTimer: ReturnType<typeof setInterval> | null = null;

	function stopMigrationPoll() {
		if (migrationTimer) {
			clearInterval(migrationTimer);
			migrationTimer = null;
		}
	}
	async function loadMigration() {
		try {
			migration = await getMigration();
			if (migration.status === 'running') {
				if (!migrationTimer) migrationTimer = setInterval(loadMigration, 2000);
			} else {
				stopMigrationPoll();
			}
		} catch {
			stopMigrationPoll();
		}
	}
	async function doMigration(action: 'start' | 'pause' | 'resume' | 'complete') {
		try {
			await migrationAction(action);
			await loadMigration();
		} catch (e) {
			reportError(e);
		}
	}

	// Migration integrity verification (separate result panel).
	let verifyResult = $state<MigrationVerifyResult | null>(null);
	let verifyError = $state<string | null>(null);
	let verifying = $state(false);
	async function doVerify() {
		verifying = true;
		verifyResult = null;
		verifyError = null;
		try {
			verifyResult = await verifyMigration(100);
		} catch (e) {
			verifyError = errorMessage(e);
		} finally {
			verifying = false;
		}
	}
	const migrationPct = $derived(
		migration && migration.total_blobs > 0
			? Math.round((migration.migrated_blobs / migration.total_blobs) * 100)
			: 0
	);
	/** Estimated minutes remaining, derived from throughput + average blob size. */
	const migrationEtaMin = $derived.by(() => {
		const m = migration;
		if (!m || m.status !== 'running' || !m.throughput_bytes_per_sec) return null;
		const remaining = m.total_blobs - m.migrated_blobs;
		if (remaining <= 0 || m.migrated_blobs <= 0) return null;
		const avgBlobSize = m.migrated_bytes / m.migrated_blobs;
		const etaSecs = (remaining * avgBlobSize) / m.throughput_bytes_per_sec;
		return Math.ceil(etaSecs / 60);
	});

	// Plugin logs
	let logsPlugin = $state<PluginInfo | null>(null);
	let logs = $state<PluginLogEntry[]>([]);
	let logsLevel = $state('');
	let logsSearch = $state('');
	let logsLoading = $state(false);
	let logsPage = $state(0);
	let logsTotal = $state(0);
	let logsLive = $state(true);
	let logStream: EventSource | null = null;

	/** Best-effort message text across the persisted (`msg`) and legacy shapes. */
	function logMsg(e: PluginLogEntry): string {
		return e.msg ?? e.message ?? '';
	}
	/** Kind column: outcome entries surface their reason, others read "log". */
	function logKind(e: PluginLogEntry): string {
		return e.kind === 'outcome' ? (e.reason ?? 'outcome') : 'log';
	}

	function stopLogStream() {
		if (logStream) {
			logStream.close();
			logStream = null;
		}
	}

	/** Open the SSE live tail for the current plugin (no-op when Live is off). */
	function startLogStream() {
		stopLogStream();
		if (!logsPlugin || !logsLive) return;
		const es = new EventSource(
			`/api/admin/plugins/${encodeURIComponent(logsPlugin.id)}/logs/stream`,
			{ withCredentials: true }
		);
		es.onmessage = (ev) => {
			try {
				onLiveLogEntry(JSON.parse(ev.data) as PluginLogEntry);
			} catch {
				/* ignore malformed frames */
			}
		};
		// Fell behind the broadcast buffer — resync from the server.
		es.addEventListener('lagged', () => void loadLogs());
		logStream = es;
	}

	/**
	 * Prepend a streamed entry, but only on the newest page and when it passes
	 * the active filter — so the live tail never fights pagination.
	 */
	function onLiveLogEntry(entry: PluginLogEntry) {
		if (logsPage !== 0) return;
		if (logsLevel && (entry.level ?? '').toLowerCase() !== logsLevel.toLowerCase()) return;
		if (logsSearch && !logMsg(entry).toLowerCase().includes(logsSearch.toLowerCase())) return;
		logs = [entry, ...logs].slice(0, LOGS_PAGE_SIZE);
		logsTotal += 1;
	}

	function toggleLive() {
		if (logsLive) startLogStream();
		else stopLogStream();
	}

	function logsPrev() {
		if (logsPage > 0) {
			logsPage--;
			void loadLogs();
		}
	}
	function logsNext() {
		if ((logsPage + 1) * LOGS_PAGE_SIZE < logsTotal) {
			logsPage++;
			void loadLogs();
		}
	}

	// Plugin detail (metadata + retention) — opened alongside logs
	let retention = $state<PluginRetention | null>(null);
	let retentionDays = $state(0);
	let retentionMb = $state(0);
	let retentionMsg = $state<string | null>(null);

	async function openLogs(p: PluginInfo) {
		logsPlugin = p;
		retention = null;
		retentionMsg = null;
		logsPage = 0;
		logsLevel = '';
		logsSearch = '';
		await Promise.all([loadLogs(), loadRetention(p.id)]);
		startLogStream();
	}

	function closeLogs() {
		stopLogStream();
		logsPlugin = null;
		logs = [];
		logsTotal = 0;
		logsPage = 0;
	}

	/** Reset to the first page (filter changed) then reload. */
	function reloadLogsFromStart() {
		logsPage = 0;
		void loadLogs();
	}
	async function loadRetention(id: string) {
		try {
			retention = await getPluginRetention(id);
			if (retention) {
				retentionDays = retention.retention_days;
				retentionMb = Math.round(retention.max_bytes / (1024 * 1024));
			}
		} catch {
			/* retention is optional — leave unset on error */
		}
	}
	async function saveRetention() {
		if (!logsPlugin) return;
		retentionMsg = null;
		if (
			!Number.isFinite(retentionDays) ||
			retentionDays < 0 ||
			!Number.isFinite(retentionMb) ||
			retentionMb < 0
		) {
			retentionMsg = t('admin.plugins_retention_invalid', 'Enter non-negative numbers.');
			return;
		}
		try {
			await savePluginRetention(logsPlugin.id, {
				retention_days: Math.round(retentionDays),
				max_bytes: Math.round(retentionMb) * 1024 * 1024
			});
			retentionMsg = t('admin.plugins_retention_saved', 'Retention saved.');
		} catch (e) {
			retentionMsg = errorMessage(e);
		}
	}
	async function purgeLogs() {
		if (!logsPlugin) return;
		if (
			!(await showConfirm(t('admin.plugins_logs_confirm_clear', 'Clear all logs for this plugin?')))
		)
			return;
		try {
			await clearPluginLogs(logsPlugin.id);
			logsPage = 0;
			await loadLogs();
		} catch (e) {
			reportError(e);
		}
	}

	// Plugin install (.zip upload)
	let installing = $state(false);
	let installMsg = $state<{ ok: boolean; text: string } | null>(null);

	async function onInstallPlugin(e: Event) {
		const input = e.currentTarget as HTMLInputElement;
		const file = input.files?.[0];
		if (!file) return;
		installing = true;
		installMsg = null;
		try {
			const info = await installPlugin(file);
			installMsg = {
				ok: true,
				text: t('admin.plugins_installed', { name: info.name }, `Installed ${info.name}.`)
			};
			await loadPlugins();
		} catch (err) {
			installMsg = { ok: false, text: errorMessage(err) };
		} finally {
			installing = false;
			input.value = '';
		}
	}
	async function loadLogs() {
		if (!logsPlugin) return;
		logsLoading = true;
		try {
			const page = await getPluginLogs(logsPlugin.id, {
				level: logsLevel,
				search: logsSearch,
				limit: LOGS_PAGE_SIZE,
				offset: logsPage * LOGS_PAGE_SIZE
			});
			logs = page.entries;
			logsTotal = page.total;
		} catch (e) {
			reportError(e);
		} finally {
			logsLoading = false;
		}
	}

	// Users
	let users = $state<User[]>([]);
	let total = $state(0);
	let pageIndex = $state(0);
	let usersError = $state<string | null>(null);
	let createOpen = $state(false);
	let createError = $state<string | null>(null);
	let creating = $state(false);
	let newUser = $state({
		username: '',
		email: '',
		password: '',
		role: 'user',
		quotaValue: 5,
		quotaUnit: (1024 ** 3) as number
	});

	// Quota edit modal
	let quotaModal = $state<{
		userId: string;
		username: string;
		value: number;
		unit: number;
	} | null>(null);

	// Reset-password modal
	let resetModal = $state<{ userId: string; username: string } | null>(null);
	let resetPassword = $state('');
	let resetError = $state<string | null>(null);
	let resetting = $state(false);

	// Plugins
	let plugins = $state<PluginInfo[]>([]);
	let pluginsAvailable = $state(true);
	let pluginsError = $state<string | null>(null);

	async function loadUsers() {
		usersError = null;
		try {
			const page = await listUsers(PAGE_SIZE, pageIndex * PAGE_SIZE);
			users = page.users;
			total = page.total;
		} catch (e) {
			usersError = errorMessage(e);
		}
	}

	async function loadPlugins() {
		pluginsError = null;
		try {
			const res = await listPlugins();
			pluginsAvailable = res.available;
			plugins = res.plugins;
		} catch (e) {
			pluginsError = errorMessage(e);
		}
	}

	function reportError(e: unknown) {
		errorToast(e);
	}

	async function copyText(text: string) {
		try {
			await navigator.clipboard.writeText(text);
			ui.notify(t('common.copied', 'Copied to clipboard'), 'success');
		} catch {
			ui.notify(t('common.copy_failed', 'Copy failed'), 'error');
		}
	}

	/** True when a settings field is locked by an OXICLOUD_* env var. */
	function isEnvLocked(overrides: string[] | undefined, field: string): boolean {
		return Array.isArray(overrides) && overrides.includes(field);
	}

	/** True for the signed-in admin's own row — guards self-destructive actions. */
	function isSelf(u: User): boolean {
		return u.id === currentAdminId;
	}
	/** OIDC/SSO-provisioned account (no local password to reset). */
	function isOidcUser(u: User): boolean {
		return !!u.auth_provider && u.auth_provider !== 'local';
	}
	/** Used-quota percentage (0 when unlimited) for the per-user progress bar. */
	function quotaPct(u: User): number {
		return u.storage_quota_bytes > 0 ? (u.storage_used_bytes / u.storage_quota_bytes) * 100 : 0;
	}

	async function toggleRole(u: User) {
		if (isSelf(u)) return;
		const role = u.role === 'admin' ? 'user' : 'admin';
		if (!(await showConfirm(t('admin.confirm_role', { role }, 'Change role to {{role}}?')))) return;
		try {
			await setUserRole(u.id, role);
			await loadUsers();
		} catch (e) {
			reportError(e);
		}
	}

	async function toggleActive(u: User) {
		if (isSelf(u) && u.active) return;
		const msg = u.active
			? t('admin.confirm_deactivate', 'Deactivate this user?')
			: t('admin.confirm_activate', 'Activate this user?');
		if (!(await showConfirm(msg))) return;
		try {
			await setUserActive(u.id, !u.active);
			await loadUsers();
		} catch (e) {
			reportError(e);
		}
	}

	function openQuota(u: User) {
		quotaModal = {
			userId: u.id,
			username: u.username || u.email,
			value:
				u.storage_quota_bytes > 0 ? Math.round((u.storage_quota_bytes / 1024 ** 3) * 10) / 10 : 0,
			unit: 1024 ** 3
		};
	}
	async function saveQuota() {
		if (!quotaModal) return;
		try {
			await setUserQuota(quotaModal.userId, Math.round(quotaModal.value * quotaModal.unit));
			quotaModal = null;
			await loadUsers();
		} catch (e) {
			reportError(e);
		}
	}

	function openReset(u: User) {
		resetModal = { userId: u.id, username: u.username || u.email };
		resetPassword = '';
		resetError = null;
	}
	async function submitReset(e: SubmitEvent) {
		e.preventDefault();
		if (!resetModal) return;
		if (resetPassword.length < 8) {
			resetError = t('admin.error_password_short', 'Password must be at least 8 characters.');
			return;
		}
		resetting = true;
		resetError = null;
		try {
			await resetUserPassword(resetModal.userId, resetPassword);
			resetModal = null;
			ui.notify(t('admin.password_reset', 'Password reset'), 'success');
		} catch (err) {
			resetError = errorMessage(err);
		} finally {
			resetting = false;
		}
	}

	async function removeUser(u: User) {
		if (isSelf(u)) return;
		if (
			!(await showConfirm(
				t('admin.confirm_delete_user', { name: u.username || u.email }, 'Delete user {{name}}?')
			))
		)
			return;
		try {
			await deleteUser(u.id);
			await loadUsers();
		} catch (e) {
			reportError(e);
		}
	}

	async function submitCreate(e: SubmitEvent) {
		e.preventDefault();
		const username = newUser.username.trim();
		const email = newUser.email.trim();
		if (username.length < 3) {
			createError = t('admin.error_username_short', 'Username must be at least 3 characters.');
			return;
		}
		if (newUser.password.length < 8) {
			createError = t('admin.error_password_short', 'Password must be at least 8 characters.');
			return;
		}
		creating = true;
		createError = null;
		try {
			await createUser({
				username,
				// Email is optional — the backend auto-generates one when blank.
				email: email || null,
				password: newUser.password,
				role: newUser.role,
				quota_bytes: Math.round(newUser.quotaValue * newUser.quotaUnit)
			});
			createOpen = false;
			newUser = {
				username: '',
				email: '',
				password: '',
				role: 'user',
				quotaValue: 5,
				quotaUnit: 1024 ** 3
			};
			await loadUsers();
		} catch (err) {
			createError = errorMessage(err);
		} finally {
			creating = false;
		}
	}

	async function togglePlugin(p: PluginInfo) {
		try {
			await setPluginEnabled(p.id, !p.enabled);
			await loadPlugins();
		} catch (e) {
			reportError(e);
		}
	}

	async function removePlugin(p: PluginInfo) {
		if (
			!(await showConfirm(
				t('admin.confirm_delete_plugin', { name: p.name }, 'Delete plugin {{name}}?')
			))
		)
			return;
		try {
			await deletePlugin(p.id);
			await loadPlugins();
		} catch (e) {
			reportError(e);
		}
	}

	function changePage(delta: number) {
		const next = pageIndex + delta;
		if (next < 0 || next * PAGE_SIZE >= total) return;
		pageIndex = next;
		void loadUsers();
	}

	// Lazy-load each tab's data on first visit.
	let loaded = $state<Record<Tab, boolean>>({
		dashboard: false,
		users: false,
		plugins: false,
		oidc: false,
		storage: false,
		smtp: false
	});

	$effect(() => {
		if (loaded[tab]) return;
		loaded[tab] = true;
		if (tab === 'dashboard') void loadDashboard();
		else if (tab === 'users') void loadUsers();
		else if (tab === 'plugins') void loadPlugins();
		else if (tab === 'oidc') void loadOidc();
		else if (tab === 'storage') {
			void loadStorage();
			void loadMigration();
		} else if (tab === 'smtp') void loadSmtp();
	});

	// Stop polling when leaving the storage tab / unmounting.
	$effect(() => {
		if (tab !== 'storage') stopMigrationPoll();
		return () => stopMigrationPoll();
	});

	// Tear down the live log stream when leaving plugins / unmounting.
	$effect(() => {
		if (tab !== 'plugins') stopLogStream();
		return () => stopLogStream();
	});
</script>

<svelte:head><title>{t('admin.title', 'Admin')} · OxiCloud</title></svelte:head>

{#snippet envBadge(on: boolean)}
	{#if on}
		<span class="badge badge--env" title={t('admin.env_locked', 'Set by an environment variable')}
			>ENV</span
		>
	{/if}
{/snippet}

<main class="admin">
	<h1>{t('admin.title', 'Admin')}</h1>

	<div class="tabs" role="tablist">
		<button role="tab" aria-selected={tab === 'dashboard'} onclick={() => (tab = 'dashboard')}>
			<Icon name="chart-pie" />
			{t('admin.dashboard', 'Dashboard')}
		</button>
		<button role="tab" aria-selected={tab === 'users'} onclick={() => (tab = 'users')}>
			<Icon name="users" />
			{t('admin.users', 'Users')}
		</button>
		<button role="tab" aria-selected={tab === 'oidc'} onclick={() => (tab = 'oidc')}>
			<Icon name="key" />
			{t('admin.oidc', 'OIDC / SSO')}
		</button>
		<button role="tab" aria-selected={tab === 'storage'} onclick={() => (tab = 'storage')}>
			<Icon name="database" />
			{t('admin.storage_tab', 'Storage')}
		</button>
		<button role="tab" aria-selected={tab === 'smtp'} onclick={() => (tab = 'smtp')}>
			<Icon name="envelope" />
			{t('admin.smtp', 'Email (SMTP)')}
		</button>
		<button role="tab" aria-selected={tab === 'plugins'} onclick={() => (tab = 'plugins')}>
			<Icon name="layer-group" />
			{t('admin.plugins', 'Plugins')}
		</button>
	</div>

	{#if tab === 'dashboard'}
		{#if dashboardError}
			<p class="status status--error">{dashboardError}</p>
		{:else if !dashboard}
			<p class="status">{t('common.loading', 'Loading…')}</p>
		{:else}
			<div class="ds-grid">
				<div class="ds-card">
					<span class="ds-num">{dashboard.total_users}</span>{t('admin.total_users', 'Total users')}
				</div>
				<div class="ds-card">
					<span class="ds-num">{dashboard.active_users}</span>{t('admin.active_users', 'Active')}
				</div>
				<div class="ds-card">
					<span class="ds-num">{dashboard.admin_users}</span>{t('admin.admin_users', 'Admins')}
				</div>
				<div class="ds-card">
					<span class="ds-num">v{dashboard.server_version}</span>{t('admin.version', 'Version')}
				</div>
			</div>

			<div class="ds-grid">
				<div class="ds-card">
					<span class="ds-flag" class:ds-flag--on={dashboard.auth_enabled}>
						{dashboard.auth_enabled
							? t('admin.enabled', 'Enabled')
							: t('admin.disabled', 'Disabled')}
					</span>
					{t('admin.auth', 'Authentication')}
				</div>
				<div class="ds-card">
					<span class="ds-flag" class:ds-flag--on={dashboard.oidc_configured}>
						{dashboard.oidc_configured ? t('admin.active', 'Active') : t('admin.off', 'Off')}
					</span>
					{t('admin.oidc', 'OIDC / SSO')}
				</div>
				<div class="ds-card">
					<span class="ds-flag" class:ds-flag--on={dashboard.quotas_enabled}>
						{dashboard.quotas_enabled
							? t('admin.enabled', 'Enabled')
							: t('admin.disabled', 'Disabled')}
					</span>
					{t('admin.quotas', 'Quotas')}
				</div>
			</div>

			{#if dashboard.users_over_quota > 0}
				<div class="card warn-card warn-card--danger">
					<Icon name="exclamation-circle" />
					<div>
						<strong class="ds-num">{dashboard.users_over_quota}</strong>
						{t('admin.over_quota', { n: dashboard.users_over_quota }, '{{n}} users over quota')}
					</div>
				</div>
			{/if}
			{#if dashboard.users_over_80_percent > 0}
				<div class="card warn-card warn-card--warn">
					<Icon name="exclamation-triangle" />
					<div>
						<strong class="ds-num">{dashboard.users_over_80_percent}</strong>
						{t(
							'admin.over_80',
							{ n: dashboard.users_over_80_percent },
							'{{n}} users over 80% quota'
						)}
					</div>
				</div>
			{/if}

			<div class="card">
				<h2>{t('admin.storage', 'Storage')}</h2>
				<div class="ds-bar">
					<div
						class="ds-fill"
						class:ds-fill--warn={dashboard.storage_usage_percent > 70}
						class:ds-fill--danger={dashboard.storage_usage_percent > 90}
						style:width="{Math.min(dashboard.storage_usage_percent, 100)}%"
					></div>
				</div>
				<p class="muted">
					{formatBytes(dashboard.total_used_bytes)} / {formatBytes(dashboard.total_quota_bytes)}
					({dashboard.storage_usage_percent.toFixed(1)}%)
				</p>
			</div>

			{#if dashboard.registration_enabled !== undefined}
				<div class="card">
					<h2>{t('admin.registration', 'Registration')}</h2>
					<label class="checkbox">
						<input
							type="checkbox"
							checked={dashboard.registration_enabled}
							onchange={(e) => toggleRegistration(e.currentTarget.checked)}
						/>
						<span>{t('admin.allow_registration', 'Allow public user registration')}</span>
					</label>
					{#if !dashboard.registration_enabled}
						<p class="alert alert--warn registration-warning">
							<Icon name="exclamation-triangle" />
							{t(
								'admin.registration_disabled_warning',
								'Public registration is disabled. Only admins can create new accounts.'
							)}
						</p>
					{/if}
				</div>
			{/if}
		{/if}
	{:else if tab === 'oidc'}
		<div class="card">
			<h2>{t('admin.oidc', 'OIDC / SSO')}</h2>
			{#if !oidc}
				<p class="status">{t('common.loading', 'Loading…')}</p>
			{:else}
				<form class="form" onsubmit={(e) => (e.preventDefault(), doSaveOidc())}>
					<label class="checkbox">
						<input type="checkbox" bind:checked={oidc.enabled} />
						<span>{t('admin.oidc_enabled', 'Enable OIDC login')}</span>
					</label>
					<label
						><span
							>{t('admin.oidc_issuer', 'Issuer URL')}{@render envBadge(
								isEnvLocked(oidc.env_overrides, 'issuer_url')
							)}</span
						>
						<input
							bind:value={oidc.issuer_url}
							placeholder="https://idp.example.com"
							disabled={isEnvLocked(oidc.env_overrides, 'issuer_url')}
						/></label
					>
					<button type="button" class="btn btn-secondary" onclick={runOidcTest}>
						<Icon name="search" />
						{t('admin.oidc_discover', 'Test / discover')}
					</button>
					{#if oidcTest}
						<div
							class="discovery-result {oidcTest.success
								? 'discovery-result--ok'
								: 'discovery-result--fail'}"
						>
							<strong>
								<Icon name={oidcTest.success ? 'check-circle' : 'times-circle'} />
								{oidcTest.message}
							</strong>
							{#if oidcTest.success}
								<dl class="kv">
									<dt>{t('admin.oidc_issuer', 'Issuer URL')}</dt>
									<dd>{oidcTest.issuer || '—'}</dd>
									<dt>{t('admin.oidc_auth_endpoint', 'Auth endpoint')}</dt>
									<dd>{oidcTest.authorization_endpoint || '—'}</dd>
								</dl>
							{/if}
						</div>
					{/if}
					<label
						><span
							>{t('admin.oidc_client_id', 'Client ID')}{@render envBadge(
								isEnvLocked(oidc.env_overrides, 'client_id')
							)}</span
						>
						<input
							bind:value={oidc.client_id}
							disabled={isEnvLocked(oidc.env_overrides, 'client_id')}
						/></label
					>
					<label
						><span
							>{t('admin.oidc_client_secret', 'Client secret')}{@render envBadge(
								isEnvLocked(oidc.env_overrides, 'client_secret')
							)}</span
						>
						<input
							type="password"
							bind:value={oidc.client_secret}
							disabled={isEnvLocked(oidc.env_overrides, 'client_secret')}
							placeholder={oidc.client_secret_set
								? t('admin.unchanged', 'Leave blank to keep current')
								: ''}
						/>
						{#if oidc.client_secret_set}
							<span class="secret-hint">
								<Icon name="check-circle" />
								{t('admin.oidc_secret_set', 'A client secret is already configured.')}
							</span>
						{/if}</label
					>
					<label
						><span
							>{t('admin.oidc_scopes', 'Scopes')}{@render envBadge(
								isEnvLocked(oidc.env_overrides, 'scopes')
							)}</span
						>
						<input
							bind:value={oidc.scopes}
							placeholder="openid profile email"
							disabled={isEnvLocked(oidc.env_overrides, 'scopes')}
						/></label
					>
					<label
						><span
							>{t('admin.oidc_provider_name', 'Provider name')}{@render envBadge(
								isEnvLocked(oidc.env_overrides, 'provider_name')
							)}</span
						>
						<input
							bind:value={oidc.provider_name}
							disabled={isEnvLocked(oidc.env_overrides, 'provider_name')}
						/></label
					>
					<label
						><span
							>{t('admin.oidc_admin_groups', 'Admin groups')}{@render envBadge(
								isEnvLocked(oidc.env_overrides, 'admin_groups')
							)}</span
						>
						<input
							bind:value={oidc.admin_groups}
							disabled={isEnvLocked(oidc.env_overrides, 'admin_groups')}
						/></label
					>
					<label class="checkbox">
						<input type="checkbox" bind:checked={oidc.auto_provision} />
						<span>{t('admin.oidc_auto_provision', 'Auto-provision users on first login')}</span>
					</label>
					<label class="checkbox">
						<input type="checkbox" bind:checked={oidc.disable_password_login} />
						<span>{t('admin.oidc_disable_pw', 'Disable password login (OIDC only)')}</span>
					</label>
					{#if oidc.callback_url}
						<p class="muted callback-row">
							{t('admin.oidc_callback', 'Callback URL')}: <code>{oidc.callback_url}</code>
							<button
								type="button"
								class="btn btn-sm btn-secondary"
								onclick={() => copyText(oidc?.callback_url ?? '')}
							>
								<Icon name="copy" />
								{t('common.copy', 'Copy')}
							</button>
						</p>
					{/if}
					{#if oidcMsg}<p class={oidcMsg.ok ? 'status--ok' : 'status--error'}>
							{oidcMsg.text}
						</p>{/if}
					<button class="btn btn-primary" type="submit" disabled={oidcSaving}>
						{t('common.save', 'Save')}
					</button>
				</form>
			{/if}
		</div>
	{:else if tab === 'storage'}
		<div class="card">
			<h2>{t('admin.storage_tab', 'Storage')}</h2>
			{#if !storage}
				<p class="status">{t('common.loading', 'Loading…')}</p>
			{:else}
				<form class="form" onsubmit={(e) => (e.preventDefault(), doSaveStorage())}>
					<label
						><span>{t('admin.storage_backend', 'Backend')}</span>
						<select bind:value={sForm.backend}>
							<option value="local">local</option>
							<option value="s3">S3</option>
						</select></label
					>
					{#if sForm.backend === 's3'}
						<label
							><span>{t('admin.storage_preset', 'Preset')}</span>
							<select bind:value={sForm.preset} onchange={applyPreset}>
								{#each Object.keys(STORAGE_PRESETS) as p (p)}<option value={p}>{p}</option>{/each}
							</select></label
						>
						<label
							><span
								>{t('admin.storage_endpoint', 'Endpoint URL')}{@render envBadge(
									isEnvLocked(storage.env_overrides, 's3_endpoint_url')
								)}</span
							>
							<input
								bind:value={sForm.endpoint}
								disabled={isEnvLocked(storage.env_overrides, 's3_endpoint_url')}
							/></label
						>
						<label
							><span
								>{t('admin.storage_bucket', 'Bucket')}{@render envBadge(
									isEnvLocked(storage.env_overrides, 's3_bucket')
								)}</span
							>
							<input
								bind:value={sForm.bucket}
								disabled={isEnvLocked(storage.env_overrides, 's3_bucket')}
							/></label
						>
						<label
							><span
								>{t('admin.storage_region', 'Region')}{@render envBadge(
									isEnvLocked(storage.env_overrides, 's3_region')
								)}</span
							>
							<input
								bind:value={sForm.region}
								disabled={isEnvLocked(storage.env_overrides, 's3_region')}
							/></label
						>
						<label
							><span
								>{t('admin.storage_access_key', 'Access key')}{@render envBadge(
									isEnvLocked(storage.env_overrides, 's3_access_key')
								)}</span
							>
							<input
								bind:value={sForm.accessKey}
								disabled={isEnvLocked(storage.env_overrides, 's3_access_key')}
								placeholder={storage.s3_access_key_set
									? t('admin.unchanged', 'Leave blank to keep current')
									: ''}
							/></label
						>
						<label
							><span
								>{t('admin.storage_secret_key', 'Secret key')}{@render envBadge(
									isEnvLocked(storage.env_overrides, 's3_secret_key')
								)}</span
							>
							<input
								type="password"
								bind:value={sForm.secretKey}
								disabled={isEnvLocked(storage.env_overrides, 's3_secret_key')}
								placeholder={storage.s3_secret_key_set
									? t('admin.unchanged', 'Leave blank to keep current')
									: ''}
							/></label
						>
						<label class="checkbox">
							<input type="checkbox" bind:checked={sForm.pathStyle} />
							<span>{t('admin.storage_path_style', 'Force path-style URLs')}</span>
						</label>
					{/if}
					{#if storageMsg}<p class={storageMsg.ok ? 'status--ok' : 'status--error'}>
							{storageMsg.text}
						</p>{/if}
					<div class="smtp-test">
						<button class="btn btn-primary" type="submit" disabled={storageBusy}
							>{t('common.save', 'Save')}</button
						>
						{#if sForm.backend === 's3'}
							<button
								type="button"
								class="btn btn-secondary"
								disabled={storageBusy}
								onclick={doTestStorage}
							>
								{t('admin.storage_test', 'Test connection')}
							</button>
						{/if}
					</div>
				</form>
				<dl class="kv">
					<dt>{t('admin.storage_current', 'Current backend')}</dt>
					<dd>{storage.current_backend ?? '—'}</dd>
					<dt>{t('admin.storage_blobs', 'Blobs')}</dt>
					<dd>{storage.total_blobs ?? '—'}</dd>
					<dt>{t('admin.storage_size', 'Stored')}</dt>
					<dd>
						{storage.total_bytes_stored != null ? formatBytes(storage.total_bytes_stored) : '—'}
					</dd>
					<dt>{t('admin.storage_dedup', 'Dedup ratio')}</dt>
					<dd>{storage.dedup_ratio != null ? `${storage.dedup_ratio.toFixed(2)}x` : '—'}</dd>
				</dl>
			{/if}
		</div>

		<div class="card">
			<h2>{t('admin.migration', 'Storage migration')}</h2>
			{#if !migration}
				<p class="status">{t('common.loading', 'Loading…')}</p>
			{:else}
				<p class="muted">{t('admin.status', 'Status')}: <strong>{migration.status}</strong></p>
				{#if migration.total_blobs > 0}
					<div class="ds-bar">
						<div class="ds-fill" style:width="{migrationPct}%"></div>
					</div>
					<p class="muted">
						{migration.migrated_blobs} / {migration.total_blobs} ({migrationPct}%) ·
						{formatBytes(migration.migrated_bytes)}
						{#if migration.throughput_bytes_per_sec && migration.status === 'running'}
							· {formatBytes(Math.round(migration.throughput_bytes_per_sec))}/s
						{/if}
						{#if migrationEtaMin != null}
							· {t('admin.mig_eta', { min: migrationEtaMin }, `~${migrationEtaMin} min remaining`)}
						{/if}
					</p>
				{/if}
				{#if migration.failed_blobs && migration.failed_blobs.length > 0}
					<details class="mig-failed">
						<summary>
							{t(
								'admin.mig_failed',
								{ n: migration.failed_blobs.length },
								`${migration.failed_blobs.length} failed blobs`
							)}
						</summary>
						<pre class="mig-failed__list">{migration.failed_blobs.join('\n')}</pre>
					</details>
				{/if}
				<div class="smtp-test">
					<!-- Start: only when no migration is active (running/paused) or completed. -->
					{#if migration.status !== 'running' && migration.status !== 'paused' && migration.status !== 'completed'}
						<button class="btn btn-primary" onclick={() => doMigration('start')}
							>{t('admin.mig_start', 'Start')}</button
						>
					{/if}
					{#if migration.status === 'running'}
						<button class="btn btn-secondary" onclick={() => doMigration('pause')}
							>{t('admin.mig_pause', 'Pause')}</button
						>
					{/if}
					{#if migration.status === 'paused'}
						<button class="btn btn-primary" onclick={() => doMigration('resume')}
							>{t('admin.mig_resume', 'Resume')}</button
						>
					{/if}
					<!-- Verify + Finalize: only once the copy phase has completed. -->
					{#if migration.status === 'completed'}
						<button class="btn btn-secondary" disabled={verifying} onclick={doVerify}>
							<Icon name="check-double" />
							{verifying
								? t('admin.mig_verifying', 'Verifying…')
								: t('admin.mig_verify', 'Verify integrity')}
						</button>
						<button class="btn btn-secondary" onclick={() => doMigration('complete')}
							>{t('admin.mig_complete', 'Finalize')}</button
						>
					{/if}
				</div>

				{#if verifyError}
					<div class="discovery-result discovery-result--fail">
						<strong><Icon name="times-circle" /> {verifyError}</strong>
					</div>
				{:else if verifyResult}
					<div
						class="discovery-result {verifyResult.passed
							? 'discovery-result--ok'
							: 'discovery-result--fail'}"
					>
						<strong>
							<Icon name={verifyResult.passed ? 'check-circle' : 'times-circle'} />
							{verifyResult.passed
								? t('admin.mig_verify_passed', 'Verification passed')
								: t('admin.mig_verify_failed', 'Verification failed')}
						</strong>
						{#if verifyResult.passed}
							<p class="muted">
								{t(
									'admin.mig_verify_summary',
									{ checked: verifyResult.sample_checked, total: verifyResult.pg_blob_count },
									'{{checked}} blobs checked, {{total}} total in database'
								)}
							</p>
						{:else}
							<p class="muted">
								{[
									verifyResult.missing_in_target.length
										? t(
												'admin.mig_verify_missing',
												{ n: verifyResult.missing_in_target.length },
												'{{n}} missing'
											)
										: '',
									verifyResult.size_mismatches.length
										? t(
												'admin.mig_verify_mismatch',
												{ n: verifyResult.size_mismatches.length },
												'{{n}} size mismatches'
											)
										: ''
								]
									.filter(Boolean)
									.join(', ')}
							</p>
						{/if}
					</div>
				{/if}
			{/if}
		</div>
	{:else if tab === 'smtp'}
		<div class="card">
			<h2>{t('admin.smtp_status', 'SMTP status')}</h2>
			{#if !smtp}
				<p class="status">{t('common.loading', 'Loading…')}</p>
			{:else}
				<dl class="kv">
					<dt>{t('admin.smtp_enabled', 'Enabled')}</dt>
					<dd>{smtp.enabled ? t('common.yes', 'Yes') : t('common.no', 'No')}</dd>
					<dt>{t('admin.smtp_host', 'Host')}</dt>
					<dd>{smtp.host || '—'}</dd>
					<dt>{t('admin.smtp_port', 'Port')}</dt>
					<dd>{smtp.port || '—'}</dd>
					<dt>TLS</dt>
					<dd>{smtp.tls || '—'}</dd>
					<dt>{t('admin.smtp_from', 'From')}</dt>
					<dd>{smtp.from || '—'}</dd>
					<dt>{t('admin.smtp_user_state', 'Auth')}</dt>
					<dd>{smtp.user_state || '—'}</dd>
				</dl>
			{/if}
		</div>
		<div class="card">
			<h2>{t('admin.smtp_test', 'Send test email')}</h2>
			<div class="smtp-test">
				<input
					type="email"
					bind:value={smtpTo}
					placeholder={t('admin.smtp_to', 'recipient@example.com')}
				/>
				<button class="btn btn-primary" disabled={smtpSending} onclick={runSmtpTest}>
					<Icon name="paper-plane" />
					{smtpSending ? t('admin.smtp_sending', 'Sending…') : t('admin.smtp_send', 'Send')}
				</button>
			</div>
			{#if smtpResult}
				{#if smtpResult.success}
					<p class="status--ok">
						<strong>{t('admin.smtp_sent', 'Test email sent.')}</strong><br />
						{t('admin.smtp_server_code', 'Server replied')}:
						<code>{smtpResult.code ?? ''} {smtpResult.message ?? ''}</code>
					</p>
				{:else}
					<p class="status--error">
						<strong>{t('admin.smtp_fail', 'Send failed.')}</strong><br />
						<code
							>{smtpResult.error || smtpResult.message || t('common.error', 'unknown error')}</code
						>
					</p>
				{/if}
			{/if}
		</div>
	{:else if tab === 'users'}
		<div class="bar">
			<button class="btn btn--primary" onclick={() => (createOpen = true)}>
				<Icon name="user-plus" />
				{t('admin.create_user', 'Create user')}
			</button>
		</div>
		{#if usersError}
			<p class="status status--error">{usersError}</p>
		{:else}
			<table class="table">
				<thead>
					<tr>
						<th>{t('admin.user', 'User')}</th>
						<th>{t('admin.role', 'Role')}</th>
						<th>{t('admin.auth', 'Auth')}</th>
						<th>{t('admin.status', 'Status')}</th>
						<th>{t('admin.quota', 'Storage usage')}</th>
						<th>{t('admin.last_login', 'Last login')}</th>
						<th></th>
					</tr>
				</thead>
				<tbody>
					{#each users as u (u.id)}
						{@const pct = quotaPct(u)}
						<tr>
							<td>
								<div class="user-cell">
									<strong>
										{u.username || u.email}
										{#if isSelf(u)}
											<span class="badge badge--self">{t('admin.you_badge', 'you')}</span>
										{/if}
									</strong>
									<span class="muted">{u.email}</span>
								</div>
							</td>
							<td>
								<span class="badge badge--{u.role === 'admin' ? 'admin' : 'user'}">
									{#if u.role === 'admin'}<Icon name="shield-alt" />{/if}
									{u.role}
								</span>
							</td>
							<td>
								{#if isOidcUser(u)}
									<span class="badge badge--oidc" title={u.auth_provider}>
										<Icon name="key" />
										{u.auth_provider}
									</span>
								{:else}
									<span class="badge badge--local">{t('admin.local', 'local')}</span>
								{/if}
							</td>
							<td>
								<span class="badge badge--{u.active ? 'active' : 'inactive'}">
									{u.active ? t('admin.active', 'Active') : t('admin.inactive', 'Inactive')}
								</span>
							</td>
							<td>
								<div class="quota-cell">
									<div class="quota-bar">
										<div
											class="quota-fill"
											class:quota-fill--warn={pct > 70}
											class:quota-fill--danger={pct > 90}
											style:width="{Math.min(pct, 100)}%"
										></div>
									</div>
									<span class="muted">
										{formatBytes(u.storage_used_bytes)} / {u.storage_quota_bytes > 0
											? formatBytes(u.storage_quota_bytes)
											: '∞'}
									</span>
								</div>
							</td>
							<td class="muted">{timeAgo(u.last_login_at)}</td>
							<td class="actions">
								<button
									class="icon-btn"
									title={t('admin.edit_quota_title', 'Edit quota')}
									aria-label={t('admin.edit_quota_title', 'Edit quota')}
									onclick={() => openQuota(u)}
								>
									<Icon name="box" />
								</button>
								{#if !isOidcUser(u)}
									<button
										class="icon-btn"
										title={t('admin.reset_password_title', 'Reset password')}
										aria-label={t('admin.reset_password_title', 'Reset password')}
										onclick={() => openReset(u)}
									>
										<Icon name="key" />
									</button>
								{/if}
								<button
									class="icon-btn"
									title={t('admin.toggle_role_title', 'Toggle admin role')}
									aria-label={t('admin.toggle_role_title', 'Toggle admin role')}
									disabled={isSelf(u)}
									onclick={() => toggleRole(u)}
								>
									<Icon name={u.role === 'admin' ? 'user' : 'crown'} />
								</button>
								<button
									class="icon-btn {u.active ? 'icon-btn--danger' : 'icon-btn--success'}"
									title={u.active
										? t('admin.deactivate_title', 'Deactivate')
										: t('admin.activate_title', 'Activate')}
									aria-label={u.active
										? t('admin.deactivate_title', 'Deactivate')
										: t('admin.activate_title', 'Activate')}
									disabled={isSelf(u) && u.active}
									onclick={() => toggleActive(u)}
								>
									<Icon name={u.active ? 'ban' : 'check'} />
								</button>
								<button
									class="icon-btn icon-btn--danger"
									title={t('admin.delete_title', 'Delete user')}
									aria-label={t('admin.delete_title', 'Delete user')}
									disabled={isSelf(u)}
									onclick={() => removeUser(u)}
								>
									<Icon name="trash-alt" />
								</button>
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
			<div class="pager">
				<button class="btn" disabled={pageIndex === 0} onclick={() => changePage(-1)}>‹</button>
				<span>{pageIndex + 1} / {Math.max(1, Math.ceil(total / PAGE_SIZE))}</span>
				<button
					class="btn"
					disabled={(pageIndex + 1) * PAGE_SIZE >= total}
					onclick={() => changePage(1)}>›</button
				>
			</div>
		{/if}
	{:else if !pluginsAvailable}
		<p class="status">{t('admin.plugins_disabled', 'The plugin subsystem is disabled.')}</p>
	{:else if pluginsError}
		<p class="status status--error">{pluginsError}</p>
	{:else}
		<div class="install-bar">
			<div>
				<strong>{t('admin.plugins_install', 'Install plugin')}</strong>
				<span class="muted"
					>{t('admin.plugins_install_hint', 'Upload a plugin bundle (.zip).')}</span
				>
			</div>
			<label class="btn btn-primary" class:disabled={installing}>
				<Icon name="cloud-upload-alt" />
				{installing
					? t('admin.plugins_installing', 'Installing…')
					: t('admin.plugins_upload', 'Upload .zip')}
				<input
					type="file"
					accept=".zip,application/zip"
					hidden
					disabled={installing}
					onchange={onInstallPlugin}
				/>
			</label>
		</div>
		{#if installMsg}
			<p class={installMsg.ok ? 'status--ok' : 'status--error'}>{installMsg.text}</p>
		{/if}
		{#if plugins.length === 0}
			<p class="status">{t('admin.no_plugins', 'No plugins installed.')}</p>
		{:else}
			<table class="table">
				<thead>
					<tr>
						<th>{t('admin.plugin', 'Plugin')}</th>
						<th>{t('admin.plugins_col_id', 'ID')}</th>
						<th>{t('admin.version', 'Version')}</th>
						<th>{t('admin.plugins_col_events', 'Events')}</th>
						<th>{t('admin.status', 'Status')}</th>
						<th></th>
					</tr>
				</thead>
				<tbody>
					{#each plugins as p (p.id)}
						<tr>
							<td>
								<div class="user-cell">
									<strong>{p.name}</strong>
									{#if p.description}<span class="muted">{p.description}</span>{/if}
								</div>
							</td>
							<td><code>{p.id}</code></td>
							<td>{p.version ?? '—'}</td>
							<td>
								{#if p.subscriptions && p.subscriptions.length > 0}
									<span class="events">{p.subscriptions.length}</span>
								{:else}
									—
								{/if}
							</td>
							<td>
								<span class="badge badge--{p.enabled ? 'active' : 'inactive'}">
									{p.enabled ? t('admin.enabled', 'Enabled') : t('admin.disabled', 'Disabled')}
								</span>
							</td>
							<td class="actions">
								<button
									class="icon-btn"
									title={t('admin.plugins_details', 'Logs & details')}
									aria-label={t('admin.plugins_details', 'Logs & details')}
									onclick={() => openLogs(p)}
								>
									<Icon name="list" />
								</button>
								<button
									class="icon-btn {p.enabled ? '' : 'icon-btn--success'}"
									title={p.enabled ? t('admin.disable', 'Disable') : t('admin.enable', 'Enable')}
									aria-label={p.enabled
										? t('admin.disable', 'Disable')
										: t('admin.enable', 'Enable')}
									onclick={() => togglePlugin(p)}
								>
									<Icon name={p.enabled ? 'pause' : 'play'} />
								</button>
								<button
									class="icon-btn icon-btn--danger"
									title={t('common.delete', 'Delete')}
									aria-label={t('common.delete', 'Delete')}
									onclick={() => removePlugin(p)}
								>
									<Icon name="trash-alt" />
								</button>
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		{/if}
	{/if}
</main>

<Modal bind:open={createOpen} title={t('admin.create_user', 'Create user')}>
	<form id="create-user-form" onsubmit={submitCreate} class="form">
		<label
			><span>{t('admin.username', 'Username')}</span>
			<input bind:value={newUser.username} minlength="3" required /></label
		>
		<label
			><span
				>{t('admin.email', 'Email')}
				<span class="muted">({t('common.optional', 'optional')})</span></span
			>
			<input
				type="email"
				bind:value={newUser.email}
				placeholder={t('admin.email_auto', 'Auto-generated if left blank')}
			/></label
		>
		<label
			><span>{t('admin.password', 'Password')}</span>
			<input type="password" bind:value={newUser.password} minlength="8" required /></label
		>
		<label
			><span>{t('admin.role', 'Role')}</span>
			<select bind:value={newUser.role}>
				<option value="user">user</option>
				<option value="admin">admin</option>
			</select></label
		>
		<label
			><span>{t('admin.quota', 'Quota')}</span>
			<div class="quota-input">
				<input type="number" min="0" step="0.1" bind:value={newUser.quotaValue} />
				<select bind:value={newUser.quotaUnit}>
					{#each QUOTA_UNITS as unit (unit.label)}<option value={unit.value}>{unit.label}</option
						>{/each}
				</select>
			</div>
			<span class="muted">{t('admin.quota_unlimited_hint', '0 = unlimited')}</span></label
		>
		{#if createError}<p class="status--error">{createError}</p>{/if}
	</form>
	{#snippet footer()}
		<button class="btn" onclick={() => (createOpen = false)}>{t('common.cancel', 'Cancel')}</button>
		<button class="btn btn--primary" type="submit" form="create-user-form" disabled={creating}>
			{creating ? t('admin.creating', 'Creating…') : t('common.create', 'Create')}
		</button>
	{/snippet}
</Modal>

<!-- Quota edit modal -->
<Modal
	open={quotaModal !== null}
	title={t('admin.edit_quota_title', 'Edit quota')}
	onclose={() => (quotaModal = null)}
>
	{#if quotaModal}
		<form
			id="quota-form"
			class="form"
			onsubmit={(e) => {
				e.preventDefault();
				void saveQuota();
			}}
		>
			<p class="muted">
				{t('admin.quota_for', 'Quota for')} <strong>{quotaModal.username}</strong>
			</p>
			<label
				><span>{t('admin.quota', 'Quota')}</span>
				<div class="quota-input">
					<input type="number" min="0" step="0.1" bind:value={quotaModal.value} />
					<select bind:value={quotaModal.unit}>
						{#each QUOTA_UNITS as unit (unit.label)}<option value={unit.value}>{unit.label}</option
							>{/each}
					</select>
				</div>
				<span class="muted">{t('admin.quota_unlimited_hint', '0 = unlimited')}</span></label
			>
		</form>
	{/if}
	{#snippet footer()}
		<button class="btn" onclick={() => (quotaModal = null)}>{t('common.cancel', 'Cancel')}</button>
		<button class="btn btn--primary" type="submit" form="quota-form">
			{t('common.save', 'Save')}
		</button>
	{/snippet}
</Modal>

<!-- Reset-password modal -->
<Modal
	open={resetModal !== null}
	title={t('admin.reset_password_title', 'Reset password')}
	onclose={() => (resetModal = null)}
>
	{#if resetModal}
		<form id="reset-pw-form" class="form" onsubmit={submitReset}>
			<p class="muted">
				{t('admin.reset_pw_for', 'New password for')} <strong>{resetModal.username}</strong>
			</p>
			<label
				><span>{t('admin.new_password', 'New password')}</span>
				<input type="password" bind:value={resetPassword} minlength="8" required /></label
			>
			{#if resetError}<p class="status--error">{resetError}</p>{/if}
		</form>
	{/if}
	{#snippet footer()}
		<button class="btn" onclick={() => (resetModal = null)}>{t('common.cancel', 'Cancel')}</button>
		<button class="btn btn--primary" type="submit" form="reset-pw-form" disabled={resetting}>
			{resetting ? t('admin.resetting', 'Resetting…') : t('admin.reset_btn', 'Reset')}
		</button>
	{/snippet}
</Modal>

<!-- Styled confirm modal (replaces native confirm) -->
<Modal
	open={confirmState !== null}
	title={t('common.confirm', 'Confirm')}
	onclose={() => resolveConfirm(false)}
>
	<p>{confirmState?.message}</p>
	{#snippet footer()}
		<button class="btn" onclick={() => resolveConfirm(false)}>{t('common.cancel', 'Cancel')}</button
		>
		<button class="btn btn--primary" onclick={() => resolveConfirm(true)}>
			{t('common.confirm', 'Confirm')}
		</button>
	{/snippet}
</Modal>

<Modal
	open={logsPlugin !== null}
	title={logsPlugin?.name ?? t('admin.plugin_logs', 'Plugin logs')}
	onclose={closeLogs}
>
	{#if logsPlugin}
		<dl class="kv plugin-meta">
			<dt>{t('admin.plugins_col_id', 'ID')}</dt>
			<dd><code>{logsPlugin.id}</code></dd>
			<dt>{t('admin.version', 'Version')}</dt>
			<dd>{logsPlugin.version ?? '—'}</dd>
			{#if logsPlugin.abi != null}
				<dt>ABI</dt>
				<dd>{logsPlugin.abi}</dd>
			{/if}
			<dt>{t('admin.plugins_col_events', 'Events')}</dt>
			<dd>
				{#if logsPlugin.subscriptions && logsPlugin.subscriptions.length > 0}
					{#each logsPlugin.subscriptions as ev (ev)}<code class="event-tag">{ev}</code>
					{/each}
				{:else}
					—
				{/if}
			</dd>
			<dt>{t('admin.status', 'Status')}</dt>
			<dd>
				<span class="badge badge--{logsPlugin.enabled ? 'active' : 'inactive'}">
					{logsPlugin.enabled ? t('admin.enabled', 'Enabled') : t('admin.disabled', 'Disabled')}
				</span>
			</dd>
		</dl>
	{/if}

	{#if retention}
		<form class="form retention-form" onsubmit={(e) => (e.preventDefault(), saveRetention())}>
			<h3>{t('admin.plugins_retention', 'Log retention')}</h3>
			<label
				><span>{t('admin.plugins_retention_days', 'Keep for (days)')}</span>
				<input type="number" min="0" bind:value={retentionDays} /></label
			>
			<label
				><span>{t('admin.plugins_retention_max', 'Max size (MB)')}</span>
				<input type="number" min="0" bind:value={retentionMb} /></label
			>
			{#if retentionMsg}<p class="muted">{retentionMsg}</p>{/if}
			<button class="btn btn-secondary" type="submit"
				>{t('admin.plugins_retention_save', 'Save retention')}</button
			>
		</form>
	{/if}

	<div class="logs-toolbar">
		<select bind:value={logsLevel} onchange={reloadLogsFromStart}>
			<option value="">{t('admin.logs_all', 'All levels')}</option>
			<option value="info">info</option>
			<option value="warn">warn</option>
			<option value="error">error</option>
		</select>
		<input
			placeholder={t('admin.logs_search', 'Search…')}
			bind:value={logsSearch}
			onkeydown={(e) => e.key === 'Enter' && reloadLogsFromStart()}
		/>
		<button class="btn btn-secondary" onclick={reloadLogsFromStart}
			>{t('common.search', 'Search')}</button
		>
		<label class="live-toggle">
			<input type="checkbox" bind:checked={logsLive} onchange={toggleLive} />
			<span>{t('admin.logs_live', 'Live')}</span>
		</label>
	</div>
	{#if logsLoading}
		<p class="status">{t('common.loading', 'Loading…')}</p>
	{:else if logs.length === 0}
		<p class="status">{t('admin.logs_empty', 'No log entries.')}</p>
	{:else}
		<div class="logs-table-wrap">
			<table class="table logs-table">
				<thead>
					<tr>
						<th>{t('admin.logs_time', 'Time')}</th>
						<th>{t('admin.logs_level', 'Level')}</th>
						<th>{t('admin.logs_kind', 'Kind')}</th>
						<th>{t('admin.logs_invocation', 'Invocation')}</th>
						<th>{t('admin.logs_message', 'Message')}</th>
					</tr>
				</thead>
				<tbody>
					{#each logs as entry, i (i)}
						<tr class="log-row log-row--{(entry.level ?? 'info').toLowerCase()}">
							<td class="log-time">{timeAgo(entry.ts ?? entry.timestamp)}</td>
							<td>
								<span class="log-level log-level--{(entry.level ?? 'info').toLowerCase()}"
									>{entry.level ?? 'info'}</span
								>
							</td>
							<td><code>{logKind(entry)}</code></td>
							<td><code class="log-inv">{entry.invocation_id ?? '—'}</code></td>
							<td class="log-msg">{logMsg(entry)}</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	{/if}
	<div class="pager logs-pager">
		<button class="btn" disabled={logsPage === 0} onclick={logsPrev}>‹</button>
		<span>
			{#if logsTotal === 0}
				{t('admin.logs_empty', 'No log entries.')}
			{:else}
				{t(
					'admin.logs_showing',
					{
						from: logsPage * LOGS_PAGE_SIZE + 1,
						to: Math.min((logsPage + 1) * LOGS_PAGE_SIZE, logsTotal),
						total: logsTotal
					},
					'Showing {{from}}–{{to}} of {{total}}'
				)}
			{/if}
		</span>
		<button class="btn" disabled={(logsPage + 1) * LOGS_PAGE_SIZE >= logsTotal} onclick={logsNext}
			>›</button
		>
	</div>
	{#snippet footer()}
		<button class="btn btn-danger" onclick={purgeLogs}
			>{t('admin.plugins_clear_logs', 'Clear logs')}</button
		>
		<button class="btn btn-secondary" onclick={closeLogs}>
			{t('common.close', 'Close')}
		</button>
	{/snippet}
</Modal>

<style>
	.logs-toolbar {
		display: flex;
		gap: var(--space-2);
		margin-bottom: var(--space-3);
	}

	.logs-toolbar input {
		flex: 1;
		padding: var(--space-2) var(--space-3);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-md);
		background: var(--color-bg-input);
		color: var(--color-text);
	}

	.live-toggle {
		display: flex;
		align-items: center;
		gap: var(--space-1);
		white-space: nowrap;
		font-size: var(--text-sm);
		color: var(--color-text-muted);
	}

	.logs-table-wrap {
		max-height: 50vh;
		overflow: auto;
	}

	.logs-table {
		font-family: var(--font-mono, monospace);
		font-size: var(--text-sm);
	}

	.log-time {
		color: var(--color-text-muted);
		white-space: nowrap;
	}

	.log-inv {
		font-size: var(--text-xs, 0.7rem);
		color: var(--color-text-muted);
	}

	.log-level {
		text-transform: uppercase;
		font-size: var(--text-xs, 0.7rem);
		font-weight: var(--weight-semibold, 600);
	}

	.log-level--error {
		color: var(--color-danger-text);
	}

	.log-level--warn {
		color: var(--color-warning-text);
	}

	.log-msg {
		word-break: break-word;
	}

	.logs-pager {
		margin-top: var(--space-3);
	}

	.card {
		background: var(--color-bg-surface);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-lg);
		padding: var(--space-5);
		margin-bottom: var(--space-4);
	}

	.card h2 {
		margin: 0 0 var(--space-3);
		font-size: 1.125rem;
	}

	.checkbox {
		display: flex;
		align-items: center;
		gap: var(--space-2);
	}

	.ds-grid {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(8rem, 1fr));
		gap: var(--space-3);
		margin-bottom: var(--space-4);
	}

	.ds-card {
		display: flex;
		flex-direction: column;
		gap: var(--space-1);
		padding: var(--space-4);
		background: var(--color-bg-surface);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-lg);
		color: var(--color-text-muted);
		font-size: var(--text-sm);
	}

	.ds-num {
		font-size: 1.5rem;
		font-weight: var(--weight-bold);
		color: var(--color-text-heading);
	}

	.ds-bar {
		height: 8px;
		background: var(--color-bg-muted);
		border-radius: var(--radius-full);
		overflow: hidden;
		margin-bottom: var(--space-2);
	}

	.ds-fill {
		height: 100%;
		background: var(--color-success-text);
	}

	.ds-fill--warn {
		background: var(--color-warning-text);
	}

	.ds-fill--danger {
		background: var(--color-danger-text);
	}

	.kv {
		display: grid;
		grid-template-columns: auto 1fr;
		gap: var(--space-1) var(--space-4);
		margin: 0;
	}

	.kv dt {
		color: var(--color-text-muted);
	}

	.kv dd {
		margin: 0;
	}

	.badge {
		display: inline-block;
		padding: 0.05rem 0.4rem;
		border-radius: var(--radius-sm);
		font-size: var(--text-xs, 0.7rem);
		font-weight: var(--weight-semibold, 600);
		line-height: 1.4;
		vertical-align: middle;
	}

	.badge--env {
		margin-left: var(--space-2);
		background: var(--color-warning-bg);
		color: var(--color-warning-text);
	}

	.badge--oidc {
		background: var(--color-info-bg);
		color: var(--color-info-text);
		text-transform: uppercase;
		display: inline-flex;
		align-items: center;
		gap: 0.25rem;
	}

	.badge--local {
		background: var(--color-bg-muted);
		color: var(--color-text-muted);
		text-transform: uppercase;
	}

	.badge--active {
		background: var(--color-success-bg);
		color: var(--color-success-text);
	}

	.badge--inactive {
		background: var(--color-bg-muted);
		color: var(--color-text-muted);
	}

	.badge--admin {
		background: var(--color-info-bg);
		color: var(--color-info-text);
		text-transform: uppercase;
		display: inline-flex;
		align-items: center;
		gap: 0.25rem;
	}

	.badge--user {
		background: var(--color-bg-muted);
		color: var(--color-text-muted);
		text-transform: uppercase;
	}

	.badge--self {
		margin-left: var(--space-1);
		background: var(--color-warning-bg);
		color: var(--color-warning-text);
		text-transform: uppercase;
	}

	/* Enabled/disabled feature flag indicator on the dashboard cards. */
	.ds-flag {
		font-size: 1.125rem;
		font-weight: var(--weight-bold);
		color: var(--color-text-muted);
	}

	.ds-flag--on {
		color: var(--color-success-text);
	}

	.warn-card {
		display: flex;
		align-items: center;
		gap: var(--space-3);
	}

	.warn-card--warn {
		border-color: var(--color-warning-text);
		color: var(--color-warning-text);
	}

	.warn-card--danger {
		border-color: var(--color-danger-text);
		color: var(--color-danger-text);
	}

	/* Per-user storage-usage progress bar in the users table. */
	.quota-cell {
		display: flex;
		flex-direction: column;
		gap: var(--space-1);
		min-width: 9rem;
	}

	.quota-bar {
		height: 6px;
		background: var(--color-bg-muted);
		border-radius: var(--radius-full);
		overflow: hidden;
	}

	.quota-fill {
		height: 100%;
		background: var(--color-success-text);
	}

	.quota-fill--warn {
		background: var(--color-warning-text);
	}

	.quota-fill--danger {
		background: var(--color-danger-text);
	}

	.quota-input {
		display: flex;
		gap: var(--space-2);
	}

	.quota-input input {
		flex: 1;
	}

	/* Icon-only row actions with hover tooltips (title attr). */
	.icon-btn {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 2rem;
		height: 2rem;
		border: 1px solid var(--color-border);
		border-radius: var(--radius-md);
		background: var(--color-bg-surface);
		color: var(--color-text);
		cursor: pointer;
	}

	.icon-btn:disabled {
		opacity: 0.45;
		cursor: not-allowed;
	}

	.icon-btn--danger {
		color: var(--color-danger-text);
	}

	.icon-btn--success {
		color: var(--color-success-text);
	}

	.secret-hint {
		display: inline-flex;
		align-items: center;
		gap: 0.25rem;
		margin-top: var(--space-1);
		font-size: var(--text-sm);
		color: var(--color-success-text);
	}

	.registration-warning {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		margin-top: var(--space-3);
	}

	.alert--warn {
		padding: var(--space-2) var(--space-3);
		border-radius: var(--radius-md);
		background: var(--color-warning-bg);
		color: var(--color-warning-text);
	}

	/* Discovery / verify result panels. */
	.discovery-result {
		margin-top: var(--space-2);
		padding: var(--space-3);
		border-radius: var(--radius-md);
		border: 1px solid var(--color-border);
	}

	.discovery-result strong {
		display: inline-flex;
		align-items: center;
		gap: var(--space-2);
	}

	.discovery-result--ok {
		border-color: var(--color-success-text);
		color: var(--color-success-text);
	}

	.discovery-result--fail {
		border-color: var(--color-danger-text);
		color: var(--color-danger-text);
	}

	.discovery-result .kv {
		margin-top: var(--space-2);
		color: var(--color-text);
	}

	.callback-row {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		flex-wrap: wrap;
	}

	.install-bar {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-3);
		padding: var(--space-3);
		border: 1px dashed var(--color-border);
		border-radius: var(--radius-md);
		margin-bottom: var(--space-3);
	}

	.install-bar .muted {
		display: block;
		font-size: var(--text-sm);
	}

	.install-bar .btn.disabled {
		opacity: 0.6;
		pointer-events: none;
	}

	.events {
		display: inline-block;
		min-width: 1.4rem;
		text-align: center;
		padding: 0 0.35rem;
		border-radius: var(--radius-pill, 999px);
		background: var(--color-bg-muted);
		color: var(--color-text-muted);
	}

	.plugin-meta {
		margin-bottom: var(--space-4);
	}

	.event-tag {
		display: inline-block;
		margin: 0 0.15rem 0.15rem 0;
		padding: 0.05rem 0.35rem;
		border-radius: var(--radius-sm);
		background: var(--color-bg-muted);
	}

	.retention-form {
		border-top: 1px solid var(--color-border);
		padding-top: var(--space-3);
		margin-bottom: var(--space-3);
	}

	.retention-form h3 {
		margin: 0 0 var(--space-2);
		font-size: 1rem;
	}

	.mig-failed {
		margin-top: var(--space-2);
	}

	.mig-failed__list {
		max-height: 12rem;
		overflow: auto;
		padding: var(--space-2);
		background: var(--color-bg-muted);
		border-radius: var(--radius-sm);
		font-size: var(--text-xs, 0.75rem);
		white-space: pre-wrap;
		word-break: break-all;
	}

	.smtp-test {
		display: flex;
		gap: var(--space-2);
	}

	.smtp-test input {
		flex: 1;
		padding: var(--space-2) var(--space-3);
		border: 1px solid var(--color-border);
		border-radius: var(--radius-md);
		background: var(--color-bg-input);
		color: var(--color-text);
	}

	.status--ok {
		color: var(--color-success-text);
	}

	.admin {
		max-width: 64rem;
		margin: 0 auto;
		padding: 1.5rem 1rem;
		display: flex;
		flex-direction: column;
		gap: 1rem;
	}

	.tabs {
		display: flex;
		gap: 0.25rem;
		border-bottom: 1px solid var(--color-border);
	}

	.tabs button {
		padding: 0.5rem 1rem;
		border: none;
		background: none;
		color: var(--color-text-muted);
		cursor: pointer;
		border-bottom: 2px solid transparent;
	}

	.tabs button[aria-selected='true'] {
		color: var(--color-text);
		border-bottom-color: var(--color-primary);
	}

	.bar {
		display: flex;
		justify-content: flex-end;
	}

	.table {
		width: 100%;
		border-collapse: collapse;
	}

	.table th,
	.table td {
		text-align: left;
		padding: 0.5rem 0.625rem;
		border-bottom: 1px solid var(--color-border);
		font-size: 0.875rem;
	}

	.user-cell {
		display: flex;
		flex-direction: column;
	}

	.muted {
		color: var(--color-text-muted);
		font-size: 0.8125rem;
	}

	.actions {
		display: flex;
		flex-wrap: wrap;
		gap: 0.5rem;
	}

	.pager {
		display: flex;
		align-items: center;
		justify-content: center;
		gap: 1rem;
	}

	.form {
		display: flex;
		flex-direction: column;
		gap: 0.75rem;
	}

	.form label {
		display: flex;
		flex-direction: column;
		gap: 0.25rem;
		font-size: 0.875rem;
	}

	.form input,
	.form select {
		padding: 0.5rem;
		border: 1px solid var(--color-border);
		border-radius: var(--radius-md);
		background: var(--color-bg-input);
		color: var(--color-text);
	}

	.btn {
		padding: 0.5rem 0.875rem;
		border: 1px solid var(--color-border);
		border-radius: var(--radius-md);
		background: var(--color-bg-surface);
		color: var(--color-text);
		cursor: pointer;
	}

	.btn--primary {
		background: var(--color-primary);
		color: var(--color-text-light);
		border-color: transparent;
	}

	.status {
		color: var(--color-text-muted);
		padding: 2rem 0;
		text-align: center;
	}

	.status--error {
		color: var(--color-danger-text);
	}

	.link-btn {
		background: none;
		border: none;
		color: var(--color-primary);
		cursor: pointer;
		font-size: 0.8125rem;
	}

	.link-btn--danger {
		color: var(--color-danger-text);
	}
</style>
