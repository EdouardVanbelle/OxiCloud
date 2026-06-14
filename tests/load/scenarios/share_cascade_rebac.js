// share_cascade_rebac.js — measures ReBAC AuthZ cascade via a direct user
// grant. The seeder grants `read` on shared_subtree.root (depth 0) to the
// grantee user; this scenario times how long the grantee takes to fetch
// folders at varying depths inside that subtree.
//
// The AuthZ recursive-CTE in `pg_acl_engine` walks up the ancestor chain
// from the requested folder until it finds a matching grant (or runs out
// of ancestors). The deeper the requested folder, the more ancestors the
// CTE has to traverse before hitting the grant on the root:
//
//   fetch_at_grant_root           → 0 ancestors walked (grant is right here)
//   fetch_as_grantee_depth4       → 4 ancestors walked
//   fetch_as_grantee_depth8       → 8 ancestors walked
//   fetch_as_grantee_depth_deep   → `load_depth` ancestors walked
//
// A regression in AuthZ cost should show up as the cascade-depth curve
// flattening or steepening — that's the value of intermediate samples.

import { check } from 'k6';
import http from 'k6/http';
import { BASE, authParams } from '../lib/http.js';
import { login } from '../lib/auth.js';
import { thresholdsFromBaseline, loadManifest } from '../lib/metrics.js';

const manifest = loadManifest();

export const options = {
  vus: 1,
  // See folder_cascade.js for the iteration-count rationale: N=100 makes
  // p99 a real percentile instead of the worst-of-the-batch.
  iterations: 100,
  thresholds: thresholdsFromBaseline('share_cascade_rebac'),
};

export function setup() {
  const adminToken = login(manifest.admin.username, manifest.admin.password);
  const granteeToken = login(manifest.grantee.username, manifest.grantee.password);
  return { adminToken, granteeToken };
}

export default function (data) {
  const { adminToken, granteeToken } = data;
  const t = manifest.shared_subtree;

  // List grants on the granted folder (admin only).
  const grantsRes = http.get(
    `${BASE}/api/grants?resource_type=folder&resource_id=${t.root}`,
    authParams(adminToken, 'share_cascade_rebac.list_grants'),
  );
  check(grantsRes, { 'list grants 200': (r) => r.status === 200 });

  // Baseline: grantee fetches the folder the grant is directly on. AuthZ
  // finds the matching grant on the first row of the CTE; zero ancestors
  // walked. This metric measures the constant overhead of an authorized
  // request — moves to the right of this metric is "cascade cost."
  const d1 = http.get(
    `${BASE}/api/folders/${t.root}/resources?resource_types=folder`,
    authParams(granteeToken, 'share_cascade_rebac.fetch_as_grantee_depth1'),
  );
  check(d1, { 'fetch root 200': (r) => r.status === 200 });

  // Grantee fetches a mid-tree folder. AuthZ walks 4 ancestors before
  // hitting the grant.
  const d4 = http.get(
    `${BASE}/api/folders/${t.depth4}/resources?resource_types=folder`,
    authParams(granteeToken, 'share_cascade_rebac.fetch_as_grantee_depth4'),
  );
  check(d4, { 'fetch depth4 200': (r) => r.status === 200 });

  // Grantee fetches a folder 8 levels under the granted root. AuthZ
  // walks 8 ancestors.
  const d8 = http.get(
    `${BASE}/api/folders/${t.depth8}/resources?resource_types=folder`,
    authParams(granteeToken, 'share_cascade_rebac.fetch_as_grantee_depth8'),
  );
  check(d8, { 'fetch depth8 200': (r) => r.status === 200 });

  // Grantee fetches the deepest descendant — full-length cascade.
  const dD = http.get(
    `${BASE}/api/folders/${t.deepest}/resources?resource_types=folder`,
    authParams(granteeToken, 'share_cascade_rebac.fetch_as_grantee_depth_deep'),
  );
  check(dD, { 'fetch deepest 200': (r) => r.status === 200 });
}
