// subject_group_nested.js — measures the worst-case AuthZ path: a user who
// is a direct member of the *innermost* group of a depth-N nested chain, and
// the grant is on the *outermost* group. Every AuthZ check has to expand the
// chain transitively. Pairs with share_cascade_rebac.js to attribute regressions
// to either the resource side (folder cascade) or the subject side (group
// expansion).

import { check } from 'k6';
import http from 'k6/http';
import { BASE, authParams } from '../lib/http.js';
import { login } from '../lib/auth.js';
import { thresholdsFromBaseline, loadManifest } from '../lib/metrics.js';

const manifest = loadManifest();

export const options = {
  vus: 1,
  iterations: 20,
  thresholds: thresholdsFromBaseline('subject_group_nested'),
};

export function setup() {
  const memberToken = login(manifest.group_member.username, manifest.group_member.password);
  return { memberToken };
}

export default function (data) {
  const { memberToken } = data;
  const t = manifest.group_subtree;

  // Group member fetches a depth-1 descendant of the group-granted root.
  const d1 = http.get(
    `${BASE}/api/folders/${t.root}/resources?resource_types=folder`,
    authParams(memberToken, 'subject_group_nested.fetch_as_member_depth1'),
  );
  check(d1, { 'fetch depth1 200': (r) => r.status === 200 });

  // Group member fetches the deepest descendant — worst-case AuthZ:
  // group_member → leaf group → mid group → root group → grant → folder root → deepest.
  const dD = http.get(
    `${BASE}/api/folders/${t.deepest}/resources?resource_types=folder`,
    authParams(memberToken, 'subject_group_nested.fetch_as_member_depth_deep'),
  );
  check(dD, { 'fetch deepest 200': (r) => r.status === 200 });
}
