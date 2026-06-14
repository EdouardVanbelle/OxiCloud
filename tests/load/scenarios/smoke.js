// smoke.js — fast PR-tier check. Verifies the load harness still builds and
// the server boots, exercising one happy-path of every HTTP verb the long
// scenarios use. No regression gate; run.sh's `smoke.sh` skips compare.mjs.

import { check, sleep } from 'k6';
import http from 'k6/http';
import { BASE, jsonParams, authParams } from '../lib/http.js';
import { login } from '../lib/auth.js';
import { thresholdsFromBaseline } from '../lib/metrics.js';

export const options = {
  vus: 1,
  iterations: 1,
  thresholds: thresholdsFromBaseline('smoke'),
};

// Admin creds match tests/load/test.env defaults — overridable via env.
const USERNAME = __ENV.K6_USERNAME || 'admin';
const PASSWORD = __ENV.K6_PASSWORD || 'TestPassword1!';

export default function () {
  // 1. login
  const token = login(USERNAME, PASSWORD, 'smoke.login');

  // 2. create a scratch folder at root
  const folderName = `smoke_${Date.now()}`;
  const createRes = http.post(
    `${BASE}/api/folders`,
    JSON.stringify({ name: folderName }),
    jsonParams(token, 'smoke.create_folder'),
  );
  check(createRes, { 'create folder 200/201': (r) => r.status === 200 || r.status === 201 });
  const folderId = createRes.json('id');

  // 3. upload a tiny file via multipart
  const fileData = http.file('hello\n', 'smoke.txt', 'text/plain');
  const uploadRes = http.post(
    `${BASE}/api/files/upload`,
    { folder_id: folderId, file: fileData },
    { headers: { Authorization: `Bearer ${token}` }, tags: { op: 'smoke.upload_tiny' } },
  );
  const uploadOk = check(uploadRes, {
    'upload 200/201': (r) => r.status === 200 || r.status === 201,
  });
  if (!uploadOk) {
    console.error(
      `upload failed: status=${uploadRes.status}, body=${uploadRes.body}, headers=${JSON.stringify(uploadRes.headers)}`,
    );
  }

  // 4. list root folders for this user
  const listRes = http.get(`${BASE}/api/folders`, authParams(token, 'smoke.list_root'));
  check(listRes, { 'list root 200': (r) => r.status === 200 });

  // 5. delete the scratch folder
  const delRes = http.del(
    `${BASE}/api/folders/${folderId}`,
    null,
    authParams(token, 'smoke.delete_folder'),
  );
  check(delRes, { 'delete 200/204': (r) => r.status === 200 || r.status === 204 });

  sleep(0.1);
}
