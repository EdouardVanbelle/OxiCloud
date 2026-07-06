# Environment Variables

Most runtime variables use the `OXICLOUD_` prefix. A few build-time or allocator variables do not.

## Server

| Variable | Default | Description |
|---|---|---|
| `OXICLOUD_STORAGE_PATH` | `./storage` | Root storage directory |
| `OXICLOUD_STATIC_PATH` | `./static` | Static files directory |
| `OXICLOUD_SERVER_PORT` | `8086` | Server port |
| `OXICLOUD_SERVER_HOST` | `127.0.0.1` | Server bind address (IPv4 or IPv6 allowed) |
| `OXICLOUD_BASE_URL` | (auto) | Public base URL for share links; defaults to `http://{host}:{port}` |
| `OXICLOUD_MAX_UPLOAD_SIZE` | `10737418240` | Whole-file size ceiling, in bytes (10 GB on 64-bit, 1 GB on 32-bit). Applies to BOTH direct PUTs (per-request body) and chunked uploads (declared `total_size`, checked upfront at session creation). |
| `OXICLOUD_DIRECT_PUT_MAX_BYTES` | `1073741824` | Per-request cap for non-chunked PUT bodies, in bytes (1 GiB). Set below `OXICLOUD_MAX_UPLOAD_SIZE` so larger files are pushed onto the chunked protocol (resumable on failure). See [Storage Fine Tuning](./storage-fine-tuning.md). |
| `OXICLOUD_CHUNK_MAX_BYTES` | `104857600` | Maximum size of a single chunked-upload PUT in bytes (100 MB). Per-chunk cap, independent of `OXICLOUD_MAX_UPLOAD_SIZE` (whole-file cap). See [Storage Fine Tuning](./storage-fine-tuning.md). |
| `OXICLOUD_CHUNK_DIR` | `{STORAGE_PATH}/.uploads` | Root directory for chunked-upload sessions (REST + NextCloud). Direct (non-chunked) uploads stream straight into the blob store and need no spool directory. Placement guidance: see [Storage Fine Tuning](./storage-fine-tuning.md). |
| `OXICLOUD_REUSE_PORT` | `false` | Enable `SO_REUSEPORT` so multiple processes can share the same port. **Disabled by default** — a second accidental instance will fail with "address already in use". Enable only for deliberate multi-worker setups (process supervisor, rolling restart). Not supported on Windows. |

## Database

| Variable | Default | Description |
|---|---|---|
| `OXICLOUD_DB_CONNECTION_STRING` | `postgres://postgres:postgres@localhost:5432/oxicloud` | PostgreSQL connection string |
| `OXICLOUD_DB_MAX_CONNECTIONS` | `20` | Max pool connections |
| `OXICLOUD_DB_MIN_CONNECTIONS` | `5` | Min pool connections |
| `OXICLOUD_DB_MAINTENANCE_MAX_CONNECTIONS` | `5` | Max connections in the isolated maintenance pool |
| `OXICLOUD_DB_MAINTENANCE_MIN_CONNECTIONS` | `1` | Min connections in the isolated maintenance pool |

## Build-Time SQLx

| Variable | Default | Description |
|---|---|---|
| `DATABASE_URL` | — | Build-time database URL for SQLx compile-time checks |

## Authentication

| Variable | Default | Description |
|---|---|---|
| `OXICLOUD_JWT_SECRET` | (auto-generated) | JWT signing secret; auto-persisted to `<STORAGE_PATH>/.jwt_secret` if unset |
| `OXICLOUD_ACCESS_TOKEN_EXPIRY_SECS` | `3600` | Access token lifetime (1 hour) |
| `OXICLOUD_REFRESH_TOKEN_EXPIRY_SECS` | `604800` | Refresh token lifetime (7 days); active sessions auto-renew on use |
| `OXICLOUD_HASH_MEMORY_COST` | `65536` | Argon2id memory cost in KiB (64 MiB) |
| `OXICLOUD_HASH_TIME_COST` | `3` | Argon2id iteration count |
| `OXICLOUD_HASH_PARALLELISM` | `2` | Argon2id parallelism lanes |
| `OXICLOUD_DISABLE_REGISTRATION` | false | Disable registration of new user accounts |

### Rate Limiting & Account Lockout

| Variable | Default | Description |
|---|---|---|
| `OXICLOUD_RATE_LIMIT_LOGIN_MAX` | `10` | Max login attempts per IP per window |
| `OXICLOUD_RATE_LIMIT_LOGIN_WINDOW_SECS` | `60` | Login rate-limit window (seconds) |
| `OXICLOUD_RATE_LIMIT_REGISTER_MAX` | `5` | Max registration attempts per IP per window |
| `OXICLOUD_RATE_LIMIT_REGISTER_WINDOW_SECS` | `3600` | Registration rate-limit window (seconds) |
| `OXICLOUD_RATE_LIMIT_REFRESH_MAX` | `20` | Max token refresh attempts per IP per window |
| `OXICLOUD_RATE_LIMIT_REFRESH_WINDOW_SECS` | `60` | Refresh rate-limit window (seconds) |
| `OXICLOUD_LOCKOUT_MAX_FAILURES` | `5` | Consecutive failed logins before account lockout |
| `OXICLOUD_LOCKOUT_DURATION_SECS` | `900` | Account lockout duration (15 minutes) |

## Feature Flags

| Variable | Default | Description |
|---|---|---|
| `OXICLOUD_ENABLE_AUTH` | `true` | Enable authentication |
| `OXICLOUD_ENABLE_USER_STORAGE_QUOTAS` | `false` | Per-user storage quotas |
| `OXICLOUD_ENABLE_FILE_SHARING` | `true` | File/folder sharing |
| `OXICLOUD_ENABLE_TRASH` | `true` | Trash / recycle bin |
| `OXICLOUD_ENABLE_SEARCH` | `true` | Full-text and metadata search |
| `OXICLOUD_ENABLE_MUSIC` | `true` | Music playlists and audio metadata |
| `OXICLOUD_EXPOSE_SYSTEM_USERS` | `true` | Expose other OxiCloud users as a read-only address book at `GET /api/address-books` |
| `OXICLOUD_ENABLE_ADMIN_INTERNAL_ENDPOINTS` | `false` | Expose `POST /api/admin/internal/trigger-sweep` and `POST /api/admin/internal/trigger-gc` — test-only synchronous triggers for the storage-usage reconciliation sweep and blob garbage collector. Used by the API test suite to assert post-delete quota convergence without waiting out the periodic ticker. Leave **off** in production: the routes return 404 even to an admin token when disabled. |
| `OXICLOUD_WEBDAV_DRIVE_LISTING_PREFIX` | `@drive` | Native WebDAV URL segment that renders the caller's drive list. Sanitized by trimming leading/trailing `/`. Three shapes: (1) default `@drive` — `/webdav/…` addresses the caller's default personal drive (back-compat), `/webdav/@drive/` returns the drive listing, `/webdav/@drive/<uuid\|name>/…` targets a specific drive. (2) empty string `""` — `/webdav/` IS the drive listing, `/webdav/<uuid\|name>/…` targets a specific drive, no default-drive shortcut. (3) any other string (e.g. `drives`) — same shape as `@drive` with that segment substituted. Only drives the caller has Read on via `role_grants` resolve. |

## Storage Backend

| Variable | Default | Description |
|---|---|---|
| `OXICLOUD_STORAGE_BACKEND` | `local` | Blob storage backend: `local`, `s3`, or `azure` |

### S3-Compatible (AWS S3, Backblaze B2, Cloudflare R2, MinIO)

Used when `OXICLOUD_STORAGE_BACKEND=s3`.

| Variable | Default | Description |
|---|---|---|
| `OXICLOUD_S3_BUCKET` | — | S3 bucket name (required) |
| `OXICLOUD_S3_REGION` | `us-east-1` | AWS region |
| `OXICLOUD_S3_ACCESS_KEY` | — | Access key ID |
| `OXICLOUD_S3_SECRET_KEY` | — | Secret access key |
| `OXICLOUD_S3_ENDPOINT_URL` | — | Custom endpoint for non-AWS providers (e.g. `https://s3.example.com`) |
| `OXICLOUD_S3_FORCE_PATH_STYLE` | `false` | Force path-style URLs (required for MinIO, R2) |

### Azure Blob Storage

Used when `OXICLOUD_STORAGE_BACKEND=azure`.

| Variable | Default | Description |
|---|---|---|
| `OXICLOUD_AZURE_ACCOUNT_NAME` | — | Storage account name (required) |
| `OXICLOUD_AZURE_ACCOUNT_KEY` | — | Storage account key |
| `OXICLOUD_AZURE_CONTAINER` | — | Blob container name (required) |
| `OXICLOUD_AZURE_SAS_TOKEN` | — | SAS token (alternative to account key) |

### Local Disk Cache for Remote Backends

A least-recently-used disk cache that can speed up repeated reads from S3 or Azure.

| Variable | Default | Description |
|---|---|---|
| `OXICLOUD_STORAGE_CACHE_ENABLED` | `false` | Enable LRU disk cache |
| `OXICLOUD_STORAGE_CACHE_MAX_SIZE` | `53687091200` | Max cache size in bytes (50 GB) |
| `OXICLOUD_STORAGE_CACHE_PATH` | `{STORAGE_PATH}/.blob-cache` | Cache directory |

### Client-Side Encryption

AES-256-GCM encryption applied to blobs before they are written to any backend.

| Variable | Default | Description |
|---|---|---|
| `OXICLOUD_STORAGE_ENCRYPTION_ENABLED` | `false` | Enable at-rest blob encryption |
| `OXICLOUD_STORAGE_ENCRYPTION_KEY` | — | Base64-encoded 32-byte encryption key; generate with `openssl rand -base64 32` |

### Retry Policy (Remote Backends)

Exponential backoff retries for transient errors on S3 and Azure.

| Variable | Default | Description |
|---|---|---|
| `OXICLOUD_STORAGE_RETRY_ENABLED` | `true` | Enable retry with exponential backoff |
| `OXICLOUD_STORAGE_RETRY_MAX_RETRIES` | `3` | Maximum retry attempts |
| `OXICLOUD_STORAGE_RETRY_INITIAL_BACKOFF_MS` | `100` | Initial backoff in milliseconds |
| `OXICLOUD_STORAGE_RETRY_MAX_BACKOFF_MS` | `10000` | Maximum backoff cap in milliseconds |
| `OXICLOUD_STORAGE_RETRY_BACKOFF_MULTIPLIER` | `2.0` | Backoff multiplier per retry |

## OIDC / SSO

See the [OIDC configuration guide](/config/oidc) for details.

| Variable | Default | Description |
|---|---|---|
| `OXICLOUD_OIDC_ENABLED` | `false` | Enable OIDC |
| `OXICLOUD_OIDC_ISSUER_URL` | — | OIDC issuer URL |
| `OXICLOUD_OIDC_CLIENT_ID` | — | Client ID |
| `OXICLOUD_OIDC_CLIENT_SECRET` | — | Client secret |
| `OXICLOUD_OIDC_REDIRECT_URI` | `http://localhost:8086/api/auth/oidc/callback` | Callback URL (must match IdP config) |
| `OXICLOUD_OIDC_SCOPES` | `openid profile email` | Requested scopes |
| `OXICLOUD_OIDC_FRONTEND_URL` | `http://localhost:8086` | Frontend URL to redirect to after login |
| `OXICLOUD_OIDC_AUTO_PROVISION` | `true` | Auto-create users on first SSO login (JIT provisioning) |
| `OXICLOUD_OIDC_ADMIN_GROUPS` | — | Comma-separated OIDC groups that grant admin role |
| `OXICLOUD_OIDC_DISABLE_PASSWORD_LOGIN` | `false` | Hide password form when OIDC is active |
| `OXICLOUD_OIDC_PROVIDER_NAME` | `SSO` | Display name for the provider shown in UI |

## WOPI (Office Editing)

See the [WOPI configuration guide](/config/wopi) for details.

| Variable | Default | Description |
|---|---|---|
| `OXICLOUD_WOPI_ENABLED` | `false` | Enable WOPI |
| `OXICLOUD_WOPI_DISCOVERY_URL` | — | Collabora/OnlyOffice discovery URL |
| `OXICLOUD_WOPI_BASE_URL` | `OXICLOUD_BASE_URL` | URL the editor uses to call OxiCloud's `/wopi/*` endpoints |
| `OXICLOUD_WOPI_PUBLIC_BASE_URL` | `OXICLOUD_WOPI_BASE_URL` | URL the browser uses to open OxiCloud's WOPI host page |
| `OXICLOUD_WOPI_SECRET` | (JWT secret) | WOPI token signing key |
| `OXICLOUD_WOPI_TOKEN_TTL_SECS` | `86400` | Token lifetime (24 hours) |
| `OXICLOUD_WOPI_LOCK_TTL_SECS` | `1800` | Lock expiration (30 minutes) |

When Collabora or OnlyOffice runs on a different hostname, set `OXICLOUD_WOPI_PUBLIC_BASE_URL` to the public OxiCloud URL that the browser can reach. If the editor reaches OxiCloud through a different internal URL, also set `OXICLOUD_WOPI_BASE_URL` for those callbacks.

## Nextcloud Compatibility

Enables the Nextcloud-compatible API layer (`/remote.php/`, `/ocs/`, `/status.php`, Login Flow v2) for clients that use the Nextcloud protocol.

| Variable | Default | Description |
|---|---|---|
| `OXICLOUD_NEXTCLOUD_ENABLED` | `false` | Enable Nextcloud compatibility layer |
| `OXICLOUD_NEXTCLOUD_INSTANCE_ID` | `ocnca` | Instance ID suffix used in `oc:id` formatting |
| `OXICLOUD_NEXTCLOUD_VERSION` | `28.0.4` | Emulated Nextcloud version reported to clients (format: `major.minor.patch`) |

## Outbound Email (SMTP)

Used by the magic-link invitation flow and the login-via-email flow. When `OXICLOUD_SMTP_HOST` is empty (the default), the feature is disabled and any endpoint that needs email returns 503.

| Variable | Default | Description |
|---|---|---|
| `OXICLOUD_SMTP_HOST` | — | SMTP server hostname or IP. Empty disables the feature. |
| `OXICLOUD_SMTP_PORT` | `587` | Submission port (587 STARTTLS, 465 implicit TLS, 25 plain) |
| `OXICLOUD_SMTP_USER` | — | SASL username. Leave empty for anonymous relay. |
| `OXICLOUD_SMTP_PASS` | — | SASL password |
| `OXICLOUD_SMTP_FROM` | — | `From:` mailbox; bare address or RFC 5322 name-address (`OxiCloud <noreply@example.com>`) |
| `OXICLOUD_SMTP_TLS` | `starttls` | Transport encryption: `starttls`, `tls`, or `none` (emits startup WARN) |

There is also `OXICLOUD_SMTP_MOCK`  (false by default), this is for test purpose only, do not activate it

### Reliability and retries

OxiCloud does **not** spool mail. Each `send()` is a single attempt: if the remote SMTP server is unreachable, slow, or temporarily refusing the message, the send fails and the error is logged — there is no in-process retry, queue, or dead-letter handling. This keeps the HTTP path fast and the binary small at the cost of durability guarantees during a relay outage.

For production deployments where you cannot afford to drop invitation mail during a brief relay outage, **point OxiCloud at a local MTA configured as a smarthost** (Postfix, OpenSMTPD, exim, or `msmtp-mta`/`nullmailer` for minimal setups). The local MTA owns the durable queue: it accepts the message from OxiCloud in milliseconds over the loopback, then retries with its own exponential backoff against your real upstream relay until the message is delivered or the queue lifetime expires.

Typical local-relay config:

```env
OXICLOUD_SMTP_HOST=127.0.0.1
OXICLOUD_SMTP_PORT=25
OXICLOUD_SMTP_TLS=none           # loopback only — never over the network
OXICLOUD_SMTP_FROM=OxiCloud <noreply@example.com>
# OXICLOUD_SMTP_USER / _PASS unset — local MTA accepts loopback unauthenticated
```

Then configure the local MTA's smarthost / relayhost to your upstream provider (SendGrid, Amazon SES, your corporate relay, etc.). Verify durability by stopping the upstream relay, sending an invitation, restarting the relay, and confirming the mail eventually arrives.

If you point `OXICLOUD_SMTP_HOST` directly at a remote SMTP server, treat the absence of retries as a documented constraint: a brief network glitch during invitation flow is a lost invite, and the recipient will need to be re-invited.

## Magic-Link Authentication

Configures the invite-by-email and login-via-email flows. Both require SMTP to be configured above.

| Variable | Default | Description |
|---|---|---|
| `OXICLOUD_MAGIC_LINK_TTL_HOURS` | `24` | Lifetime of a freshly-minted magic-link token, in hours |
| `OXICLOUD_ALLOW_EXTERNAL_USERS` | `true` | Kill switch for the whole flow. `false` makes `POST /api/grants` reject `subject.type = "email"` for unknown addresses and `POST /api/auth/magic-link/send` return its uniform stub without issuing a token. |
| `OXICLOUD_EXTERNAL_EMAIL_DOMAINS` | — | Comma-separated allowlist of email domains accepted when minting a new external user (case-insensitive, exact match on the post-`@` part). Empty = any domain is allowed, subject to `OXICLOUD_ALLOW_EXTERNAL_USERS`. Subdomains must be listed explicitly: `partner.com` does NOT match `eng.partner.com`. Example: `partner-a.com,partner-b.io`. |
| `OXICLOUD_NOTIFY_INTERNAL_USERS_ON_SHARE` | `true` | Operator-level kill switch for the **plain-notification** email arm — the "Alice shared 'Project Alpha' with you" mail that fires when the recipient is a password user or OIDC user (i.e. not magic-link eligible). `false` suppresses the arm entirely; internal users discover new shares only at next login. A coarser knob than the per-user `auth.users.notify_on_share` column; when this is `false` the user-level opt-in does not matter. External-user magic-link **first-invitations** are unaffected and always send. |

## Internationalization (server-rendered surfaces)

Server-rendered HTML pages (magic-link landing, error pages) and outbound transactional emails go through the backend i18n layer. The set of available locales is **discovered at boot** by listing `static/locales/*.json` — no rebuild needed to add a 17th locale.

| Variable | Default | Description |
|---|---|---|
| `OXICLOUD_DEFAULT_LOCALE` | `en` | Fallback locale used when no stronger signal is available. Must match one of the locales under `static/locales/`; startup fails fast if you set it to a code with no corresponding JSON file. |

The resolution priority differs by surface:

- **HTML pages (anonymous, e.g. magic-link landing)** — `?lang=xx` query override, then the browser's `Accept-Language` header (q-weighted, with primary-tag fallback so `fr-FR` resolves to `fr` when no `fr-FR.json` is shipped), then this default.
- **Emails to a known user** — the user's `preferred_locale` column (set via OIDC `locale` claim at JIT or via the UI language switcher), then this default.
- **Emails to a brand-new external user being invited** — the inviter's `preferred_locale` (inheritance at row-creation), then this default.

Today's shipped locales: `ar, de, en, es, fa, fr, hi, it, ja, ko, nl, pl, pt, ru, zh, zh-TW`. Missing translations on a non-English locale automatically fall back to English at the key level — adding a new locale with even a few translated keys works without manual gap-filling.

## Trusted Proxy

| Variable | Default | Description |
|---|---|---|
| `OXICLOUD_TRUST_PROXY_CIDR` | — | Comma-separated list of trusted proxy CIDRs; enables `X-Forwarded-For` / `X-Real-IP` extraction for those source IPs |
| `OXICLOUD_TRUST_PROXY_HEADERS` | — | **Deprecated.** Use `OXICLOUD_TRUST_PROXY_CIDR` instead |

Example: `OXICLOUD_TRUST_PROXY_CIDR=127.0.0.1/32,10.0.0.0/8,172.16.0.0/12`

## Allocator Tuning

These variables are read directly by **mimalloc**, not by OxiCloud's config parser.

| Variable | Default | Description |
|---|---|---|
| `MIMALLOC_PURGE_DELAY` | `0` | Delay in ms before freed memory is returned to the OS (`0` = immediately, recommended for Docker) |
| `MIMALLOC_ALLOW_LARGE_OS_PAGES` | `0` | Enable 2 MiB huge pages (`0` = off, recommended for Docker to avoid THP RSS inflation) |

## Internal Defaults (not configurable via env)

| Parameter | Default |
|---|---|
| File cache TTL | 60 s |
| Directory cache TTL | 120 s |
| Max cache entries | 10 000 |
| Large file threshold | 100 MB |
| Streaming chunk size | 1 MB |
| Max parallel chunks | 8 |
| Trash retention | 30 days |
| Argon2id memory cost | 64 MiB |
| Argon2id time cost | 3 iterations |
| Nextcloud Login Flow v2 TTL | 600 s |
