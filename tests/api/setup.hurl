# =============================================================
# OxiCloud – First-time admin setup
# =============================================================
# Creates the initial admin account. Only works once: the
# endpoint is disabled after the first admin exists.
#
# Run:
#   hurl --variables-file tests/api/hurl.vars --test tests/api/setup.hurl
# =============================================================


# ─────────────────────────────────────────────────────────────
# Step 1 – Create the first admin account
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/setup
Content-Type: application/json
{
  "username": "{{username}}",
  "email": "{{email}}",
  "password": "{{password}}"
}

HTTP 201
[Asserts]
jsonpath "$.username" == "{{username}}"
jsonpath "$.email" == "{{email}}"
jsonpath "$.role" == "admin"


# ─────────────────────────────────────────────────────────────
# Step 2 – Verify the endpoint is now disabled (admin exists)
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/setup
Content-Type: application/json
{
  "username": "another",
  "email": "another@example.com",
  "password": "irrelevant"
}

HTTP 403
[Asserts]
jsonpath "$.error_type" == "SystemAlreadyInitialized"
