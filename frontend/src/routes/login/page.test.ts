import { it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';

const { goto, pageState, session } = vi.hoisted(() => {
	// `setUser` mirrors the real SessionStore method: sets the user and
	// runs `ensureActiveUser` (localStorage cleanup on account switch).
	// Tests don't care about the cleanup; the mock just assigns.
	const store: { user: unknown; setUser: (u: unknown) => void } = {
		user: null,
		setUser(u) {
			store.user = u;
		}
	};
	return {
		goto: vi.fn(),
		pageState: { url: new URL('http://localhost/login') } as { url: URL },
		session: store
	};
});
vi.mock('$app/navigation', () => ({ goto }));
vi.mock('$app/state', () => ({ page: pageState }));
vi.mock('$lib/stores/session.svelte', () => ({ session }));
vi.mock('$lib/api/endpoints/auth', () => ({
	exchangeOidcCode: vi.fn(),
	fetchMe: vi.fn(),
	getOidcProviders: vi.fn(),
	getAuthStatus: vi.fn(),
	login: vi.fn(),
	register: vi.fn(),
	sendMagicLink: vi.fn(),
	setupAdmin: vi.fn()
}));

import * as auth from '$lib/api/endpoints/auth';
import LoginPage from './+page.svelte';

const m = (fn: unknown) => fn as ReturnType<typeof vi.fn>;

beforeEach(() => {
	vi.clearAllMocks();
	pageState.url = new URL('http://localhost/login');
	session.user = null;
	m(auth.fetchMe).mockResolvedValue(null);
	m(auth.getOidcProviders).mockResolvedValue({ providers: [] });
	m(auth.getAuthStatus).mockResolvedValue({ initialized: true });
});

it('logs in and redirects', async () => {
	m(auth.login).mockResolvedValue({ user: { id: '1' } });
	render(LoginPage);
	await screen.findByTestId('login-form');
	await fireEvent.input(screen.getByTestId('login-username-input'), { target: { value: 'admin' } });
	await fireEvent.input(screen.getByTestId('login-password-input'), { target: { value: 'pw' } });
	await fireEvent.click(screen.getByTestId('login-submit-btn'));
	await waitFor(() => expect(auth.login).toHaveBeenCalled());
});

it('exchanges an oidc code on mount and redirects', async () => {
	pageState.url = new URL('http://localhost/login?oidc_code=abc');
	m(auth.exchangeOidcCode).mockResolvedValue({ id: '1' });
	render(LoginPage);
	await waitFor(() => expect(auth.exchangeOidcCode).toHaveBeenCalledWith('abc'));
	await waitFor(() => expect(goto).toHaveBeenCalled());
});

it('skips the form when already authenticated', async () => {
	m(auth.fetchMe).mockResolvedValue({ id: '1' });
	render(LoginPage);
	await waitFor(() => expect(goto).toHaveBeenCalled());
});

it('enters setup mode on a fresh install', async () => {
	m(auth.getAuthStatus).mockResolvedValue({ initialized: false });
	render(LoginPage);
	await screen.findByTestId('login-setup-form');
});

it('sends a magic link', async () => {
	m(auth.sendMagicLink).mockResolvedValue('sent');
	render(LoginPage);
	await screen.findByTestId('login-form');
	await fireEvent.click(screen.getByTestId('login-magic-toggle-btn'));
	await fireEvent.input(screen.getByTestId('login-magic-email-input'), {
		target: { value: 'a@b.test' }
	});
	await fireEvent.click(screen.getByTestId('login-magic-send-btn'));
	await waitFor(() => expect(auth.sendMagicLink).toHaveBeenCalledWith('a@b.test'));
});

it('registers a new account', async () => {
	m(auth.register).mockResolvedValue(undefined);
	render(LoginPage);
	await screen.findByTestId('login-form');
	await fireEvent.click(screen.getByTestId('login-to-register-btn'));
	await fireEvent.input(screen.getByTestId('login-register-username-input'), {
		target: { value: 'u' }
	});
	await fireEvent.input(screen.getByTestId('login-register-email-input'), {
		target: { value: 'u@b.test' }
	});
	await fireEvent.input(screen.getByTestId('login-register-password-input'), {
		target: { value: 'TestPassword1!' }
	});
	await fireEvent.input(screen.getByTestId('login-register-confirm-input'), {
		target: { value: 'TestPassword1!' }
	});
	await fireEvent.click(screen.getByTestId('login-register-submit-btn'));
	await waitFor(() => expect(auth.register).toHaveBeenCalled());
});

it('shows an error message when login fails', async () => {
	m(auth.login).mockRejectedValue(new Error('bad credentials'));
	render(LoginPage);
	await screen.findByTestId('login-form');
	await fireEvent.input(screen.getByTestId('login-username-input'), { target: { value: 'admin' } });
	await fireEvent.input(screen.getByTestId('login-password-input'), { target: { value: 'wrong' } });
	await fireEvent.click(screen.getByTestId('login-submit-btn'));
	await waitFor(() => expect(screen.getByText('bad credentials')).toBeTruthy());
});

it('rejects a registration with mismatched passwords without calling the API', async () => {
	render(LoginPage);
	await screen.findByTestId('login-form');
	await fireEvent.click(screen.getByTestId('login-to-register-btn'));
	await fireEvent.input(screen.getByTestId('login-register-username-input'), {
		target: { value: 'u' }
	});
	await fireEvent.input(screen.getByTestId('login-register-password-input'), {
		target: { value: 'TestPassword1!' }
	});
	await fireEvent.input(screen.getByTestId('login-register-confirm-input'), {
		target: { value: 'Different1!' }
	});
	await fireEvent.click(screen.getByTestId('login-register-submit-btn'));
	expect(auth.register).not.toHaveBeenCalled();
});

it('creates the first administrator in setup mode', async () => {
	m(auth.getAuthStatus).mockResolvedValue({ initialized: false });
	m(auth.setupAdmin).mockResolvedValue(undefined);
	render(LoginPage);
	await screen.findByTestId('login-setup-form');
	await fireEvent.input(screen.getByTestId('login-setup-email-input'), {
		target: { value: 'admin@x.test' }
	});
	await fireEvent.input(screen.getByTestId('login-setup-password-input'), {
		target: { value: 'TestPassword1!' }
	});
	await fireEvent.input(screen.getByTestId('login-setup-confirm-input'), {
		target: { value: 'TestPassword1!' }
	});
	await fireEvent.click(screen.getByTestId('login-setup-submit-btn'));
	await waitFor(() =>
		expect(auth.setupAdmin).toHaveBeenCalledWith('admin@x.test', 'TestPassword1!')
	);
});

it('renders an SSO sign-in link when an OIDC provider is configured', async () => {
	m(auth.getOidcProviders).mockResolvedValue({
		enabled: true,
		authorize_endpoint: 'https://idp.test/auth',
		provider_name: 'Acme SSO',
		password_login_enabled: true
	});
	render(LoginPage);
	const sso = await screen.findByTestId('login-oidc-btn');
	expect(sso.getAttribute('href')).toBe('https://idp.test/auth');
});
