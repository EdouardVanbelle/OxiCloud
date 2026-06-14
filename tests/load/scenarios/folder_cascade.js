// folder_cascade.js — measures the read-path of `GET /api/folders/{id}/resources?resource_types=folder`
// at four depths against a pre-seeded tree. Captures how listing cost scales
// with ltree depth (cf. `idx_folders_lpath` GiST index). Mid-depth samples
// (depth4, depth8) fall back to `deepest` when the seeded tree is shallower
// than that depth — see load-seed.rs::build_subtree.

import { check } from 'k6';
import http from 'k6/http';
import { BASE, authParams } from '../lib/http.js';
import { login } from '../lib/auth.js';
import { thresholdsFromBaseline, loadManifest } from '../lib/metrics.js';

const manifest = loadManifest();

export const options = {
  vus: 1,
  // 100 iterations gives p99 statistical meaning: at N=100, p99 = position 99,
  // representing one bad sample out of a hundred — a real percentile rather
  // than the worst-of-the-batch. At N=25 (the typical k6 example default),
  // p99 ≈ max, dominated by single-sample kernel/scheduler noise.
  iterations: 100,
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

  const r8 = http.get(
    `${BASE}/api/folders/${t.depth8}/resources?resource_types=folder`,
    authParams(token, 'folder_cascade.list_depth8'),
  );
  check(r8, { 'list depth8 200': (r) => r.status === 200 });

  const rD = http.get(
    `${BASE}/api/folders/${t.deepest}/resources?resource_types=folder`,
    authParams(token, 'folder_cascade.list_depth_deep'),
  );
  check(rD, { 'list deepest 200': (r) => r.status === 200 });
}
