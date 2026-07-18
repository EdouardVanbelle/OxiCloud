// Fake OpenID Connect Identity Provider for the OxiCloud OIDC
// integration test (tests/oidc/oidc.hurl).
//
// Wraps panva/node-oidc-provider — a spec-compliant OP — with a
// minimal Node http front-end that auto-resolves every interaction
// (login and consent) for a hard-coded test user. We don't wrap with
// our own Koa instance because oidc-provider ships a bundled Koa that
// the response prototype-checks against; layering another Koa around
// it triggers `vary: res argument is required` on the first request.
//
// What we get from the library that we'd otherwise hand-roll:
//   * Discovery (.well-known/openid-configuration)
//   * JWKS endpoint + RS256-signed JWTs
//   * PKCE S256 verification
//   * Authorization code lifecycle
//   * Refresh token + id_token + access_token shapes
//
// What we get FOR FREE when we later add coverage for:
//   * Back-channel logout — flip features.backchannelLogout.enabled
//   * RP-initiated logout — flip features.rpInitiatedLogout.enabled
//   * Token revocation (RFC 7009) — flip features.revocation.enabled
//   * Token introspection (RFC 7662) — flip features.introspection.enabled
//
// Each future OIDC feature is a config flag in this file rather than
// new Rust protocol code to maintain.

import http from 'node:http';
import { URL } from 'node:url';
import { default as Provider } from 'oidc-provider';

// ── Configuration knobs ─────────────────────────────────────────────────
const ISSUER = process.env.FAKE_IDP_ISSUER || 'http://localhost:1080';
const PORT = parseInt(process.env.FAKE_IDP_PORT || '1080', 10);
const TEST_USER_SUB = 'oidc-test-user';
const TEST_USER_USERNAME = 'oidc_user';
const TEST_USER_EMAIL = 'oidc@example.com';
// Full claim set pinned to deterministic values so the Hurl test can
// assert that JIT provisioning (auth_application_service.rs:2257)
// stores each one verbatim. Keep the claim names matching the OIDC
// `IdTokenClaims` struct in src/infrastructure/services/oidc_service.rs.
const TEST_USER_NAME = 'OIDC Test User';
const TEST_USER_GIVEN_NAME = 'OIDC';
const TEST_USER_FAMILY_NAME = 'Test';
// `picture` is the OIDC claim; OxiCloud persists it as `User.image`
// (a URL or data URI). We use a stable HTTP URL so a simple equality
// check works in the Hurl assertion.
const TEST_USER_PICTURE = 'https://example.com/oidc-test-user.png';
// Group claim — paired with OXICLOUD_OIDC_ADMIN_GROUPS=admin-users in
// server-with-oidc.env. The JIT path intersects this list against the
// configured admin groups; a non-empty intersection escalates the new
// user's role from `user` to `admin`. This is the typical SSO pattern
// every Authentik/Keycloak/Entra deployment uses to map IdP groups to
// app roles.
const TEST_USER_GROUPS = ['admin-users'];

// ── Runtime-toggleable state for negative tests ────────────────────────
// `email_verified` is normally true; the test flips it to false via
// `POST /control/email-verified/false` to drive OxiCloud's anti-takeover
// rejection branch (auth_application_service.rs: only `email_verified`
// callers reach JIT-provisioning), then flips back. Module-level state
// because oidc-provider doesn't pass test-specific context into the
// claims() callback.
let emailVerifiedState = true;

const configuration = {
  clients: [
    {
      client_id: 'oxicloud-test',
      client_secret: 'test-client-secret-not-used-in-prod',
      // 8087: automated tests/oidc/oidc.hurl suite. 8090: human-run
      // tests/oidc/run-manual-sso-only.sh (SSO-only auto-redirect check).
      redirect_uris: [
        'http://localhost:8087/api/auth/oidc/callback',
        'http://localhost:8090/api/auth/oidc/callback',
      ],
      grant_types: ['authorization_code'],
      response_types: ['code'],
      token_endpoint_auth_method: 'client_secret_post',
    },
  ],

  pkce: { required: () => true, methods: ['S256'] },

  claims: {
    openid: ['sub'],
    email: ['email', 'email_verified'],
    // `profile` is the standard scope OxiCloud requests
    // (OXICLOUD_OIDC_SCOPES in server-with-oidc.env). It covers every
    // claim the JIT-provisioning code in auth_application_service.rs
    // reads except email — name + given/family + picture +
    // preferred_username + groups all ride here.
    profile: [
      'name',
      'given_name',
      'family_name',
      'preferred_username',
      'picture',
      'groups',
    ],
  },

  async findAccount(_ctx, sub) {
    if (sub !== TEST_USER_SUB) return undefined;
    return {
      accountId: sub,
      // Return EVERY claim the OIDC client could ask for. The provider
      // filters by the consented scope before issuing — values not in
      // a granted scope are dropped from the ID token / userinfo.
      async claims() {
        return {
          sub: TEST_USER_SUB,
          email: TEST_USER_EMAIL,
          email_verified: emailVerifiedState,
          name: TEST_USER_NAME,
          given_name: TEST_USER_GIVEN_NAME,
          family_name: TEST_USER_FAMILY_NAME,
          preferred_username: TEST_USER_USERNAME,
          picture: TEST_USER_PICTURE,
          groups: TEST_USER_GROUPS,
        };
      },
    };
  },

  features: {
    // Turn off the dev login/consent UI; we own the interaction route.
    devInteractions: { enabled: false },
  },

  // Put scope-implied claims (name, given_name, family_name,
  // preferred_username, picture, email, …) directly into the ID token
  // instead of keeping them at /userinfo only.
  //
  // OxiCloud's OIDC client (auth_application_service.rs:2085) only
  // calls /userinfo when the ID token lacks `email` — with the email
  // scope granted the ID token DOES carry email, so userinfo never
  // runs, and the default (conformIdTokenClaims: true) means `picture`
  // would silently vanish during JIT provisioning. Setting this to
  // `false` mirrors what most real-world IdPs (Authentik, Keycloak's
  // default profile) do for browser SSO clients.
  conformIdTokenClaims: false,

  // Point every interaction at our auto-resolver below.
  interactions: {
    url(_ctx, interaction) {
      return `/auto/${interaction.uid}`;
    },
  },

  cookies: {
    keys: ['fake-idp-cookie-key-not-a-real-secret'],
  },
};

const provider = new Provider(ISSUER, configuration);
provider.proxy = false;

// `provider.callback()` is an http-compatible request handler.
// We intercept /auto/<uid> ourselves and forward everything else.
const oidcHandler = provider.callback();

// `/control/*` paths are test-only hooks the Hurl suite uses to
// flip IdP-side state between flows (e.g. force email_verified=false
// to exercise OxiCloud's anti-takeover rejection branch). Kept on the
// SAME port as the OIDC endpoints so we don't have to thread two ports
// through every test config. Never used in production-shaped flows.
function handleControl(req, res) {
  const url = new URL(req.url, ISSUER);
  if (req.method === 'POST' && url.pathname === '/control/email-verified/true') {
    emailVerifiedState = true;
    res.statusCode = 200;
    res.setHeader('content-type', 'application/json');
    return res.end(JSON.stringify({ email_verified: true }));
  }
  if (req.method === 'POST' && url.pathname === '/control/email-verified/false') {
    emailVerifiedState = false;
    res.statusCode = 200;
    res.setHeader('content-type', 'application/json');
    return res.end(JSON.stringify({ email_verified: false }));
  }
  res.statusCode = 404;
  res.setHeader('content-type', 'application/json');
  return res.end(JSON.stringify({ error: 'no such control endpoint' }));
}

// One-line per-request log — useful when a future test fails
// mysteriously ("did OxiCloud actually call /me?" / "is the
// /authorize redirect hitting the right URL?"). Kept because it's
// low-noise and makes the next debugging session 10x easier; the
// payload-dumping diagnostics that helped land the
// `image`-missing-from-INSERT fix (UserPgRepository::create_user)
// have been stripped.
const server = http.createServer(async (req, res) => {
  // eslint-disable-next-line no-console
  console.log(`[fake-idp] ${req.method} ${req.url}`);
  if (req.url.startsWith('/control/')) return handleControl(req, res);

  try {
    const url = new URL(req.url, ISSUER);
    const autoMatch = url.pathname.match(/^\/auto\/[^/]+\/?$/);

    if (autoMatch) {
      return await handleAuto(req, res);
    }

    return oidcHandler(req, res);
  } catch (e) {
    // eslint-disable-next-line no-console
    console.error('[fake-idp] unhandled error:', e);
    if (!res.headersSent) {
      res.statusCode = 500;
      res.setHeader('content-type', 'application/json');
      res.end(JSON.stringify({ error: 'internal', detail: String(e) }));
    }
  }
});

// ── Auto-approve handler ───────────────────────────────────────────────
// The library redirects /authorize to /auto/<uid>. We pull the
// interaction state, sign the test user in (prompt=login), then grant
// every requested claim+scope (prompt=consent). The provider issues
// the authorization code and 302s back to OxiCloud's callback.
async function handleAuto(req, res) {
  const details = await provider.interactionDetails(req, res);
  const {
    prompt: { name },
    params,
  } = details;

  if (name === 'login') {
    return provider.interactionFinished(
      req,
      res,
      { login: { accountId: TEST_USER_SUB } },
      { mergeWithLastSubmission: false },
    );
  }

  if (name === 'consent') {
    const grant = new provider.Grant({
      accountId: TEST_USER_SUB,
      clientId: params.client_id,
    });
    if (params.scope) grant.addOIDCScope(params.scope);
    // Explicitly grant every profile claim OxiCloud reads at JIT
    // provisioning (see src/application/services/auth_application_service.rs
    // around line 2257). `addOIDCClaims` is additive to whatever the
    // scope already implies, so listing them here is belt-and-braces
    // for keeping the claim set complete.
    grant.addOIDCClaims([
      'email',
      'email_verified',
      'name',
      'given_name',
      'family_name',
      'preferred_username',
      'picture',
    ]);
    const grantId = await grant.save();
    return provider.interactionFinished(
      req,
      res,
      { consent: { grantId } },
      { mergeWithLastSubmission: true },
    );
  }

  res.statusCode = 400;
  res.setHeader('content-type', 'application/json');
  res.end(JSON.stringify({ error: 'unsupported_prompt', prompt: name }));
}

server.listen(PORT, () => {
  // eslint-disable-next-line no-console
  console.log(`[fake-idp] listening on ${ISSUER} (test user sub=${TEST_USER_SUB})`);
});
