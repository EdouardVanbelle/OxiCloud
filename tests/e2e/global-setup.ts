import { TEST_ADMIN } from './scenarios/helpers';

export default async function globalSetup() {
  const res = await fetch('http://localhost:8087/api/setup', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      username: TEST_ADMIN.username,
      email:    TEST_ADMIN.email,
      password: TEST_ADMIN.password,
    }),
  });

  // 409 = admin already exists (idempotent across retries)
  if (!res.ok && res.status !== 409) {
    throw new Error(`Admin setup failed: ${res.status} ${await res.text()}`);
  }
}
