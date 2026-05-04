import { defineConfig, devices } from '@playwright/test';

export default defineConfig({
  testDir: './scenarios',
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 2 : 0,
  workers: process.env.CI ? 1 : undefined,
  reporter: process.env.CI ? [['github'], ['html']] : 'html',

  globalSetup: require.resolve('./global-setup'),

  use: {
    baseURL: 'http://localhost:8087',
    trace: 'on-first-retry',
  },

  expect: {
    toHaveScreenshot: { maxDiffPixelRatio: 0.02 },
  },

  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'] },
    },
    {
      name: 'firefox',
      use: { ...devices['Desktop Firefox'] },
    },
  ],

  webServer: {
    command: process.env.CI ? `${process.env.GITHUB_WORKSPACE}/target/debug/oxicloud` : 'cargo run',
    url: 'http://localhost:8087',
    timeout: 600_000,
    reuseExistingServer: false,
    cwd: '../..',
    stdout: 'pipe',
    stderr: 'pipe',
    env: {
      DATABASE_URL: 'postgres://oxicloud_test:oxicloud_test@localhost:5433/oxicloud_test',
      OXICLOUD_SERVER_PORT: 8087,
      OXICLOUD_DB_CONNECTION_STRING: 'postgres://oxicloud_test:oxicloud_test@localhost:5433/oxicloud_test',
      OXICLOUD_STORAGE_PATH: './tests/e2e/storage',
      OXICLOUD_STATIC_PATH: './static',
      OXICLOUD_JWT_SECRET: 'test-secret-do-not-use-in-prod-minimum-32-chars',
      OXICLOUD_ENABLE_AUTH: 'true',
      OXICLOUD_ENABLE_TRASH: 'true',
      OXICLOUD_ENABLE_SEARCH: 'true',
      OXICLOUD_ENABLE_FILE_SHARING: 'true',
      OXICLOUD_WOPI_ENABLED: 'false',
      OXICLOUD_OIDC_ENABLED: 'false',
      RUST_LOG: 'warn',
    },
  },
});
