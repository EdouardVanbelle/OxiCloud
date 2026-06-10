// share_cascade_rebac.js — measures ReBAC AuthZ cascade via a direct user
// grant. The seeder grants `read` on shared_subtree.root to the grantee user;
// this scenario times how long the grantee takes to fetch folders at varying
// depths inside that subtree (each fetch triggers the AuthZ recursive-CTE in
// pg_acl_engine). It also times listing the grants on the root.

import { check } from 'k6';
import http from 'k6/http';
import { BASE, authParams } from '../lib/http.js';
import { login } from '../lib/auth.js';
import { thresholdsFromBaseline, loadManifest } from '../lib/metrics.js';

const manifest = loadManifest();

export const options = {
  vus: 1,
  iterations: 20,
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

  // Grantee fetches the granted root's depth-1 child (the AuthZ check
  // expands the grant via the subtree).
  const d1 = http.get(
    `${BASE}/api/folders/${t.root}/resources?resource_types=folder`,
    authParams(granteeToken, 'share_cascade_rebac.fetch_as_grantee_depth1'),
  );
  check(d1, { 'fetch depth1 200': (r) => r.status === 200 });

  // Grantee fetches the deepest descendant — AuthZ walks the longest cascade.
  const dD = http.get(
    `${BASE}/api/folders/${t.deepest}/resources?resource_types=folder`,
    authParams(granteeToken, 'share_cascade_rebac.fetch_as_grantee_depth_deep'),
  );
  check(dD, { 'fetch deepest 200': (r) => r.status === 200 });
}
