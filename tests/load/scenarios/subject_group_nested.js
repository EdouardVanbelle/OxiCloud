// subject_group_nested.js — measures the worst-case AuthZ path: a user who
// is a direct member of the *innermost* group of a depth-N nested chain, and
// the grant is on the *outermost* group. Every AuthZ check has to expand the
// chain transitively. Pairs with share_cascade_rebac.js to attribute regressions
// to either the resource side (folder cascade) or the subject side (group
// expansion).
//
// Same depth-gradient idea as share_cascade_rebac: measure AuthZ cost at
// depth 1 / 4 / 8 / deep so we can see WHERE in the chain a regression
// lands. The subject-side expansion (group_member → leaf → mid → root group
// → grant) runs once per request regardless of folder depth, but the
// resource-side cascade (folder ancestors walked to find the grant) grows
// linearly with depth — so depth4/depth8/deep moving together while depth1
// stays flat would point at the resource side; all four moving together
// would point at the group-expansion path.

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
  thresholds: thresholdsFromBaseline('subject_group_nested'),
};

export function setup() {
  const memberToken = login(manifest.group_member.username, manifest.group_member.password);
  return { memberToken };
}

export default function (data) {
  const { memberToken } = data;
  const t = manifest.group_subtree;

  // Baseline: group member fetches the folder the grant is directly on.
  // Subject-side expansion runs (user → leaf → mid → root group → grant);
  // resource-side walk is zero ancestors. Captures the constant subject-
  // expansion overhead.
  const d1 = http.get(
    `${BASE}/api/folders/${t.root}/resources?resource_types=folder`,
    authParams(memberToken, 'subject_group_nested.fetch_as_member_depth1'),
  );
  check(d1, { 'fetch root 200': (r) => r.status === 200 });

  // Mid-tree: 4 folder ancestors walked + subject expansion.
  const d4 = http.get(
    `${BASE}/api/folders/${t.depth4}/resources?resource_types=folder`,
    authParams(memberToken, 'subject_group_nested.fetch_as_member_depth4'),
  );
  check(d4, { 'fetch depth4 200': (r) => r.status === 200 });

  // 8 folder ancestors walked + subject expansion.
  const d8 = http.get(
    `${BASE}/api/folders/${t.depth8}/resources?resource_types=folder`,
    authParams(memberToken, 'subject_group_nested.fetch_as_member_depth8'),
  );
  check(d8, { 'fetch depth8 200': (r) => r.status === 200 });

  // Worst case: full-depth folder cascade × full-chain subject expansion.
  // group_member → leaf group → mid group → root group → grant → folder root → deepest.
  const dD = http.get(
    `${BASE}/api/folders/${t.deepest}/resources?resource_types=folder`,
    authParams(memberToken, 'subject_group_nested.fetch_as_member_depth_deep'),
  );
  check(dD, { 'fetch deepest 200': (r) => r.status === 200 });
}
