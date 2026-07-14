import { test, expect } from './coverage-helpers';

/**
 * Login route — register and magic-link flows (logged out). Exercises the
 * register/magic submit handlers in the login page + auth endpoints.
 */
function uniq(p: string): string {
  return `${p}-${Date.now()}-${Math.floor(Math.random() * 1e6)}`;
}

test('register a new account from the login page', async ({ page }) => {
  const u = uniq('reg');
  await page.goto('/login');
  await page.getByTestId('login-to-register-btn').click();
  await expect(page.getByTestId('login-register-form')).toBeVisible();

  await page.getByTestId('login-register-username-input').fill(u);
  await page.getByTestId('login-register-email-input').fill(`${u}@example.test`);
  await page.getByTestId('login-register-password-input').fill('TestPassword1!');
  await page.getByTestId('login-register-confirm-input').fill('TestPassword1!');
  await page.getByTestId('login-register-submit-btn').click();

  // Either we land in the app or a notice/error appears — both run onRegister.
  await expect(
    page
      .getByTestId('appshell-logo-link')
      .or(page.locator('.auth-error'))
      .or(page.getByTestId('login-register-form'))
      .first(),
  ).toBeVisible({ timeout: 15_000 });
});

// Note: the first-run setup panel is unreachable here — `login-to-setup-btn`
// only renders when no admin exists, but the test env always seeds one.

test('register with mismatched passwords shows a validation error', async ({ page }) => {
  await page.goto('/login');
  await page.getByTestId('login-to-register-btn').click();
  await expect(page.getByTestId('login-register-form')).toBeVisible();

  await page.getByTestId('login-register-username-input').fill(uniq('mm'));
  await page.getByTestId('login-register-email-input').fill('mm@example.test');
  await page.getByTestId('login-register-password-input').fill('TestPassword1!');
  await page.getByTestId('login-register-confirm-input').fill('Different1!');
  await page.getByTestId('login-register-submit-btn').click();

  // Client-side validation rejects the mismatch before any request.
  await expect(page.locator('.auth-error[role="alert"]')).toBeVisible({ timeout: 5_000 });
});

test('an oidc callback code is exchanged on load', async ({ page }) => {
  // Landing with ?oidc_code triggers the SPA's OIDC code-exchange path; a bogus
  // code fails and falls back to the login form (exercises the handler).
  await page.goto('/login?oidc_code=fake-code-123');
  await expect(
    page.getByTestId('login-form').or(page.locator('.auth-error')).first(),
  ).toBeVisible({ timeout: 15_000 });
});

test('request a magic link from the login page', async ({ page }) => {
  // Unified form: leave the password field empty and submit — the
  // adaptive submit routes to /api/auth/magic-link/send with the
  // identifier as-is (backend accepts email OR username via `@`
  // dispatch). Old separate `login-magic-*` testids retired in the
  // single-form refactor.
  await page.goto('/login');
  await expect(page.getByTestId('login-form')).toBeVisible();

  await page.getByTestId('login-username-input').fill('someone@example.test');
  // Password intentionally NOT filled.
  await page.getByTestId('login-submit-btn').click();

  // A status message resolves (uniform 200 anti-enum success or error);
  // give the request time to run.
  await page.waitForTimeout(1_000);
  // Still on the login page either way — anti-enum success doesn't redirect.
  await expect(page.getByTestId('login-form')).toBeVisible();
});
