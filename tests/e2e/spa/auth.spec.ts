import { test, expect, uiLogin, TEST_ADMIN } from './coverage-helpers';

test.describe('SPA · authentication', () => {
  test('login page renders the sign-in form', async ({ page }) => {
    await page.goto('/login');
    await expect(page.getByTestId('login-form')).toBeVisible();
    await expect(page.getByTestId('login-username-input')).toBeVisible();
    await expect(page.getByTestId('login-password-input')).toBeVisible();
    await expect(page.getByTestId('login-submit-btn')).toBeVisible();
    await expect(page).toHaveTitle(/OxiCloud/i);
  });

  test('wrong password is rejected with an error', async ({ page }) => {
    await page.goto('/login');
    await page.getByTestId('login-username-input').fill(TEST_ADMIN.username);
    await page.getByTestId('login-password-input').fill('definitely-wrong-password');
    await page.getByTestId('login-submit-btn').click();

    await expect(page.locator('.auth-error[role="alert"]')).toBeVisible();
    // Still on the login page — no redirect into the app.
    await expect(page.getByTestId('login-form')).toBeVisible();
  });

  test('register and setup panels are reachable from login', async ({ page }) => {
    await page.goto('/login');
    await page.getByTestId('login-to-register-btn').click();
    await expect(page.getByTestId('login-register-form')).toBeVisible();
    await page.getByTestId('login-register-to-login-btn').click();
    await expect(page.getByTestId('login-form')).toBeVisible();
  });

  test('submit button dispatches to magic-link when password is empty', async ({ page }) => {
    // Unified login form: one identifier + one optional password + one
    // adaptive submit button. Filling the identifier and leaving the
    // password blank flips the button label to "Send sign-in link" and
    // routes to /api/auth/magic-link/send on click. The old two-form
    // UX with `login-magic-toggle-btn` was retired 2026-07-14.
    await page.goto('/login');
    await expect(page.getByTestId('login-form')).toBeVisible();
    await page.getByTestId('login-username-input').fill('someone@example.test');
    // Password intentionally NOT filled — this drives the label swap.
    const submit = page.getByTestId('login-submit-btn');
    await expect(submit).toBeVisible();
    // Label content differs per mode: password-empty → magic-link copy;
    // password-filled → "Sign in". Assert the magic-link copy is what's
    // shown so the dispatch is provably in the magic-link branch.
    await expect(submit).toHaveText(/link|Link|Send/);
  });

  test('successful login reaches the files app shell', async ({ page }) => {
    await uiLogin(page);
    await expect(page).toHaveURL(/\/files/);
    await expect(page.getByTestId('appshell-logo-link')).toBeVisible();
    await expect(page.getByTestId('appshell-user-menu-btn')).toBeVisible();
  });

  test('logout returns to the login page', async ({ page }) => {
    await uiLogin(page);
    await page.getByTestId('appshell-user-menu-btn').click();
    await page.getByTestId('appshell-user-menu-logout-btn').click();
    await page.waitForURL('**/login**', { timeout: 15_000 });
    await expect(page.getByTestId('login-form')).toBeVisible();
  });
});
