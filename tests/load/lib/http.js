// Shared HTTP helpers for OxiCloud k6 load tests.
// Centralises the base URL and adds the Bearer token + JSON Content-Type
// headers expected by every protected endpoint.

import http from 'k6/http';

/**
 * Base URL of the OxiCloud server under test.
 * Read from K6_BASE_URL (set by run.sh), defaulting to the load-suite port.
 */
export const BASE = __ENV.K6_BASE_URL || 'http://localhost:8088';

/**
 * Build a request params object with auth + JSON headers and a `tag` so
 * the response's metrics are isolated under `<scenario>.<op>`.
 *
 * @param {string} token  Bearer token from auth.login()
 * @param {string} op     Metric tag, e.g. 'folder_cascade.list_depth8'
 */
export function jsonParams(token, op) {
  return {
    headers: {
      Authorization: `Bearer ${token}`,
      'Content-Type': 'application/json',
    },
    tags: { op },
  };
}

/**
 * Same as jsonParams but without a body content-type — for GETs and DELETEs.
 */
export function authParams(token, op) {
  return {
    headers: { Authorization: `Bearer ${token}` },
    tags: { op },
  };
}

export const httpClient = http;
