// folder_cascade.js — measures the read-path of `GET /api/folders/{id}/resources?resource_types=folder`
// at three depths against a pre-seeded tree. Captures how listing cost scales
// with ltree depth (cf. `idx_folders_lpath` GiST index).

import { check } from 'k6';
import http from 'k6/http';
import { BASE, authParams } from '../lib/http.js';
import { login } from '../lib/auth.js';
import { thresholdsFromBaseline, loadManifest } from '../lib/metrics.js';

const manifest = loadManifest();

export const options = {
  vus: 1,
  iterations: 25,
  thresholds: thresholdsFromBaseline('folder_cascade'),
};

// One per-VU login. K6 calls setup() once across the whole test, default()
// `iterations` times per VU. Logging in inside default() would dominate the
// per-iter cost; we hand the token down through the `data` arg.
export function setup() {
  const token = login(manifest.admin.username, manifest.admin.password);
  return { token };
}

export default function (data) {
  const { token } = data;
  const t = manifest.shared_subtree;

  const r1 = http.get(
    `${BASE}/api/folders/${t.root}/resources?resource_types=folder`,
    authParams(token, 'folder_cascade.list_depth1'),
  );
  check(r1, { 'list depth1 200': (r) => r.status === 200 });

  const r4 = http.get(
    `${BASE}/api/folders/${t.depth4}/resources?resource_types=folder`,
    authParams(token, 'folder_cascade.list_depth4'),
  );
  check(r4, { 'list depth4 200': (r) => r.status === 200 });

  const rD = http.get(
    `${BASE}/api/folders/${t.deepest}/resources?resource_types=folder`,
    authParams(token, 'folder_cascade.list_depth_deep'),
  );
  check(rD, { 'list deepest 200': (r) => r.status === 200 });
}
