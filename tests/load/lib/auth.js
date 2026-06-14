// Login helper for OxiCloud k6 load tests.
// Returns the JWT access token from POST /api/auth/login.

import { check, fail } from 'k6';
import http from 'k6/http';
import { BASE } from './http.js';

/**
 * Log in via the public auth endpoint and return the bearer token.
 *
 * `body` shape matches `auth_handler.rs::login` — `{username, password}`.
 * The response carries `access_token` plus refresh state we don't need here.
 *
 * The `op` tag should be scenario-qualified (e.g. 'smoke.login') so the
 * recorded metric matches the corresponding key in baseline/baseline.json.
 * Bare 'login' is fine for ad-hoc scripts that aren't regression-gated.
 *
 * @param {string} username
 * @param {string} password
 * @param {string} [op='login']
 * @returns {string}
 */
export function login(username, password, op = 'login') {
  const res = http.post(
    `${BASE}/api/auth/login`,
    JSON.stringify({ username, password }),
    { headers: { 'Content-Type': 'application/json' }, tags: { op } },
  );

  const ok = check(res, {
    'login 200': (r) => r.status === 200,
    'login has access_token': (r) => !!r.json('access_token'),
  });
  if (!ok) {
    fail(`login failed for ${username}: status=${res.status}, body=${res.body}`);
  }
  return res.json('access_token');
}
