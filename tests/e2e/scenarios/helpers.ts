import { Page, expect } from '@playwright/test';

export const TEST_ADMIN = {
  username: 'admin',
  email: 'testadmin@example.com',
  password: 'TestPassword1!',
};

/**
 * Log in as the test admin and wait until the main app is ready.
 */
export async function loginAsAdmin(page: Page) {
  await goToLoginPage(page);
  await page.locator('#login-username').fill(TEST_ADMIN.username);
  await page.locator('#login-password').fill(TEST_ADMIN.password);
  await page.locator('#login-panel button[type="submit"]').click();
  await expect(page.locator('#sidebar')).toBeVisible({ timeout: 15_000 });
}

/**
 * Navigate to `/` and land on the login panel, handling the language selector
 * if it appears (fresh localStorage). The admin account is guaranteed to exist
 * because globalSetup created it before any test ran.
 */
export async function goToLoginPage(page: Page) {
  await page.goto('/');

  // Both panels start with .hidden — wait for JS to reveal one.
  await page.waitForSelector('#language-panel:not(.hidden), #login-panel:not(.hidden)');

  if (await page.locator('#language-panel').isVisible()) {
    await page.locator('#language-continue').click();
  }

  await expect(page.locator('#login-panel')).toBeVisible();
}
