# WebDAV

OxiCloud exposes a fully RFC 4918 compliant WebDAV interface at `/webdav/`. It works with all major file managers and sync clients.

## Base URL

```
https://your-server:8086/webdav/
```

## Drives in the URL

A user can own multiple [drives](/guide/drives) (one personal + any
number of shared drives they've been added to). The WebDAV URL scheme
lets you address them all, and the operator can choose between two
layouts via `OXICLOUD_WEBDAV_DRIVE_LISTING_PREFIX` (default `"@drive"`):

**Default — `"@drive"` sigil.** Bare `/webdav/…` addresses your default
personal drive, keeping single-drive clients working with zero config.
Explicit drive listing lives under the sigil.

| URL | Target |
|---|---|
| `/webdav/` | Your default personal drive (back-compat) |
| `/webdav/Documents/report.pdf` | A file inside your default drive |
| `/webdav/@drive/` | Directory listing of every drive you can read |
| `/webdav/@drive/<uuid-or-name>/…` | A specific drive by UUID or display name |

**Empty prefix (`""`) — flat layout.** `/webdav/` IS the drive listing.
Every drive appears as a top-level entry. No hidden default.

| URL | Target |
|---|---|
| `/webdav/` | Directory listing of every drive you can read |
| `/webdav/<uuid-or-name>/…` | A specific drive by UUID or display name |

Set `OXICLOUD_WEBDAV_DRIVE_LISTING_PREFIX=""` for the flat layout.
Any non-empty value replaces the sigil (e.g. `"drives"` gives you
`/webdav/drives/<selector>/…`).

**Trade-off with the empty prefix**: recursive DAV clients (Cyberduck,
Finder, rclone default, NC desktop) will mirror ALL drives you can
read, which can be a lot of storage. The `@drive` sigil keeps the
default drive as the client's sync root and puts the picker behind an
opt-in URL. Pick the empty prefix only when you want explicit
multi-drive visibility.

**Folder name collision note.** A user could name a folder `@drive`
inside their default drive; that folder would then mask the drive
picker for that user under the default sigil. Rare enough to be
accepted; the sigil is renameable via the env var above if it becomes
an issue.

## Authentication

HTTP Basic Authentication:

```
Authorization: Basic base64(username:app_password)
```

::: warning Use an app password, NOT your account password
DAV clients authenticate with an **app password** — a distinct, revocable,
scoped credential. Your regular OxiCloud account password (used in the
web login) will always be refused on `/webdav/`, `/caldav/`, and
`/carddav/`.

Why: app passwords are the only credential that works uniformly across
all account types (password, magic-link-only, OIDC-linked), and they can
be revoked individually without touching your account password.

**Generate an app password:** open OxiCloud in your browser, go to
**Profile → App Passwords**, click *Create*, name it (e.g. "Thunderbird
laptop"), and copy the token shown once. Use your username + that token
in every DAV client.
:::

::: tip HTTPS
Always use HTTPS in production — Basic auth sends credentials in every
request.
:::

## Supported Methods

| Method | Description |
|--------|-------------|
| `PROPFIND` | List directory contents / get file properties |
| `GET` | Download a file |
| `PUT` | Upload a file |
| `MKCOL` | Create a folder |
| `MOVE` | Move or rename a file/folder |
| `COPY` | Copy a file/folder |
| `DELETE` | Delete a file/folder |
| `LOCK` / `UNLOCK` | File locking |

## Common Operations

### List a directory

Use `PROPFIND` with a `Depth` header:

```http
PROPFIND /webdav/projects/ HTTP/1.1
Depth: 1
Content-Type: application/xml

<?xml version="1.0" encoding="utf-8" ?>
<D:propfind xmlns:D="DAV:">
  <D:allprop/>
</D:propfind>
```

Successful directory listings return `207 Multi-Status`.

### Download a file

```http
GET /webdav/projects/document.pdf HTTP/1.1
Authorization: Basic base64(username:app_password)
```

### Upload or replace a file

```http
PUT /webdav/projects/document.pdf HTTP/1.1
Content-Type: application/pdf

<file bytes>
```

### Create a folder

```http
MKCOL /webdav/projects/new-folder HTTP/1.1
```

### Move or copy

```http
MOVE /webdav/old-location.pdf HTTP/1.1
Destination: https://your-server/webdav/new-location.pdf
```

```http
COPY /webdav/original.pdf HTTP/1.1
Destination: https://your-server/webdav/copy.pdf
```

### Delete a resource

```http
DELETE /webdav/projects/document.pdf HTTP/1.1
```

## Client Setup

### Windows Explorer

1. Open **This PC** → **Map network drive**
2. Enter: `https://your-server:8086/webdav/`
3. Check **Connect using different credentials**
4. Enter your OxiCloud username and an [app password](#authentication)

### macOS Finder

1. **Go** → **Connect to Server** (⌘K)
2. Enter: `https://your-server:8086/webdav/`
3. Enter your OxiCloud username and an [app password](#authentication)

### Linux (Nautilus / Files)

1. Open Files → **Other Locations**
2. In the address bar, type: `davs://your-server:8086/webdav/`
3. Enter your OxiCloud username and an [app password](#authentication)

### Linux (Dolphin / KDE)

1. In the address bar, type: `webdavs://your-server:8086/webdav/`
2. Enter your OxiCloud username and an [app password](#authentication)

### Command Line (curl)

`user:apppw` below means your OxiCloud username + the app-password token
you generated in *Profile → App Passwords* (not your account password).

```bash
# List root directory
curl -u user:apppw -X PROPFIND https://your-server:8086/webdav/ \
  -H "Depth: 1"

# Download a file
curl -u user:apppw https://your-server:8086/webdav/document.pdf -o document.pdf

# Upload a file
curl -u user:apppw -T localfile.txt https://your-server:8086/webdav/remotefile.txt

# Create a folder
curl -u user:apppw -X MKCOL https://your-server:8086/webdav/new-folder/
```

## Streaming PROPFIND

OxiCloud streams PROPFIND responses, so listing directories with thousands of files doesn't consume excessive memory.

## Integration Notes

- the WebDAV handler is only an HTTP adapter; file and folder operations still go through the same application services used by the REST API
- HTTP Basic Authentication is supported for DAV clients, while authorization rules remain the same as the rest of OxiCloud
- delete operations integrate with trash when the trash feature is enabled

## Troubleshooting

- **401 Unauthorized on every request?** You're almost certainly using
  your account password instead of an app password. Open OxiCloud in
  your browser → *Profile* → *App Passwords* → *Create*, then use the
  token shown once (with your username) in your client. See
  [Authentication](#authentication) above.
- Always use the `/webdav/` base path
- Prefer HTTPS because WebDAV uses Basic Authentication
- On Windows, make sure the `WebClient` service is enabled
- OxiCloud rejects path traversal segments such as `.` and `..` at the HTTP boundary
