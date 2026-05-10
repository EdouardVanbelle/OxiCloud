# API functional tests

Tests are written using [Hurl](https://hurl.dev) — a plain-text, CLI-first HTTP testing tool.

## Prerequisites

- [Hurl](https://hurl.dev/docs/installation.html) ≥ 4.0  
  Install: `cargo install hurl` or via your package manager

## Configuration

Edit `.env` to match your local instance. This file is the single source of
truth: `run.sh` sources it for shell variables and passes it to Hurl as
`--variables-file`.

```
base_url=http://localhost:8087
username=admin
email=admin@example.com
password=TestPassword1!
```

## Running the tests

```bash
# First-time setup (run once on a fresh instance)
hurl --variables-file tests/api/hurl.vars --test tests/api/setup.hurl

# Contacts CRUD scenario
hurl --variables-file tests/api/hurl.vars --test tests/api/contacts.hurl

# All scenarios at once
hurl --variables-file tests/api/hurl.vars --test tests/api/setup.hurl tests/api/contacts.hurl

# With full request/response output
hurl --variables-file tests/api/hurl.vars --test --verbose tests/api/contacts.hurl

# Generate an HTML report
hurl --variables-file tests/api/hurl.vars --test --report-html /tmp/hurl-report tests/api/contacts.hurl
```

## Test files

| File | Description |
|---|---|
| `setup.hurl` | One-time admin account creation; also asserts the endpoint is locked afterwards |
| `contacts.hurl` | Full contacts CRUD scenario (13 steps, see below) |
| `.env` | Variables: `base_url`, `username`, `email`, `password` — used by both Hurl and `run.sh` |

## Scenario: `contacts.hurl`

| Step | Description |
|---|---|
| 1 | Login – capture JWT token |
| 2 | List address books – assert system book is present and read-only |
| 3 | Create personal address book – capture `book_id` |
| 4 | List contacts in new book – assert empty |
| 5 | Create contact John Doe – capture `contact_id` |
| 6 | List contacts – assert exactly 1 result with John Doe's id |
| 7 | Get John Doe – assert all fields, capture `ETag` |
| 8 | Update John Doe (nickname, org, notes) with `If-Match` – assert new values, capture refreshed `ETag` |
| 9 | Delete John Doe with `If-Match` |
| 10 | List contacts – assert empty again |
| 11 | Delete personal address book |
| 12 | List address books – assert `book_id` no longer present |
| 13 | List system address book – assert non-empty collection of OxiCloud users |

## Legacy bash tests

`test.sh` and `common.sh` are the original curl/bash scripts kept for reference.
Run them with `bash tests/api/test.sh` from the repo root (requires `jq`).
