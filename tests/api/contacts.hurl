# =============================================================
# OxiCloud – Contacts API end-to-end scenario
# =============================================================
# Run:
#   hurl --variables-file tests/api/hurl.vars --test tests/api/contacts.hurl
#
# Variables required (see hurl.vars):
#   base_url   – e.g. http://localhost:8087
#   username   – OxiCloud username
#   password   – OxiCloud password
# =============================================================


# ─────────────────────────────────────────────────────────────
# Step 1 – Login and capture the JWT token
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/auth/login
Content-Type: application/json
{
  "username": "{{username}}",
  "password": "{{password}}"
}

HTTP 200
[Captures]
token: jsonpath "$.access_token"
[Asserts]
jsonpath "$.access_token" isString
jsonpath "$.token_type" == "Bearer"


# ─────────────────────────────────────────────────────────────
# Step 2 – List address books (at least the system book)
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/address-books
Authorization: Bearer {{token}}

HTTP 200
[Asserts]
jsonpath "$" isCollection
# System address book must always be present
jsonpath "$[?(@.id == 'system')].is_system" == true
jsonpath "$[?(@.id == 'system')].is_readonly" == true


# ─────────────────────────────────────────────────────────────
# Step 3 – Create a personal address book
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/address-books
Authorization: Bearer {{token}}
Content-Type: application/json
{
  "name": "Personal",
  "description": "Personal address book created by Hurl tests",
  "is_public": false
}

HTTP 201
[Captures]
book_id: jsonpath "$.id"
[Asserts]
jsonpath "$.id" isString
jsonpath "$.name" == "Personal"
jsonpath "$.description" == "Personal address book created by Hurl tests"
jsonpath "$.is_public" == false
jsonpath "$.is_system" == false
jsonpath "$.is_readonly" == false


# ─────────────────────────────────────────────────────────────
# Step 4 – List contacts in the new book – must be empty
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/address-books/{{book_id}}/contacts
Authorization: Bearer {{token}}

HTTP 200
[Asserts]
jsonpath "$" isCollection
jsonpath "$" count == 0


# ─────────────────────────────────────────────────────────────
# Step 5 – Create contact John Doe
# ─────────────────────────────────────────────────────────────
POST {{base_url}}/api/address-books/{{book_id}}/contacts
Authorization: Bearer {{token}}
Content-Type: application/json
{
  "first_name": "John",
  "last_name": "Doe",
  "full_name": "John Doe",
  "email": [
    {
      "email": "john.doe@example.com",
      "type": "work",
      "is_primary": true
    }
  ],
  "phone": [
    {
      "number": "+1-555-0100",
      "type": "mobile",
      "is_primary": true
    }
  ]
}

HTTP 201
[Captures]
contact_id: jsonpath "$.id"
[Asserts]
jsonpath "$.id" isString
jsonpath "$.address_book_id" == {{book_id}}
jsonpath "$.first_name" == "John"
jsonpath "$.last_name" == "Doe"
jsonpath "$.full_name" == "John Doe"
jsonpath "$.email" count == 1
jsonpath "$.email[0].email" == "john.doe@example.com"
jsonpath "$.email[0].type" == "work"
jsonpath "$.email[0].is_primary" == true
jsonpath "$.phone" count == 1
jsonpath "$.phone[0].number" == "+1-555-0100"
jsonpath "$.phone[0].type" == "mobile"
jsonpath "$.phone[0].is_primary" == true


# ─────────────────────────────────────────────────────────────
# Step 6 – List contacts – exactly 1 result, must be John Doe
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/address-books/{{book_id}}/contacts
Authorization: Bearer {{token}}

HTTP 200
[Asserts]
jsonpath "$" count == 1
jsonpath "$[*].id" contains {{contact_id}}
jsonpath "$[0].first_name" == "John"
jsonpath "$[0].last_name" == "Doe"
jsonpath "$[0].full_name" == "John Doe"


# ─────────────────────────────────────────────────────────────
# Step 7 – Get John Doe by id and verify all fields
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/address-books/{{book_id}}/contacts/{{contact_id}}
Authorization: Bearer {{token}}

HTTP 200
[Captures]
etag: header "ETag"
[Asserts]
header "ETag" exists
jsonpath "$.id" == {{contact_id}}
jsonpath "$.address_book_id" == {{book_id}}
jsonpath "$.first_name" == "John"
jsonpath "$.last_name" == "Doe"
jsonpath "$.full_name" == "John Doe"
jsonpath "$.email[0].email" == "john.doe@example.com"
jsonpath "$.email[0].type" == "work"
jsonpath "$.email[0].is_primary" == true
jsonpath "$.phone[0].number" == "+1-555-0100"
jsonpath "$.phone[0].type" == "mobile"


# ─────────────────────────────────────────────────────────────
# Step 8 – Update John Doe: add nickname, notes, and organisation
#           Uses If-Match for optimistic concurrency
# ─────────────────────────────────────────────────────────────
PUT {{base_url}}/api/address-books/{{book_id}}/contacts/{{contact_id}}
Authorization: Bearer {{token}}
Content-Type: application/json
If-Match: {{etag}}
{
  "first_name": "John",
  "last_name": "Doe",
  "full_name": "John Doe",
  "nickname": "JD",
  "organization": "ACME Corp",
  "notes": "Updated via Hurl test",
  "email": [
    {
      "email": "john.doe@example.com",
      "type": "work",
      "is_primary": true
    }
  ],
  "phone": [
    {
      "number": "+1-555-0100",
      "type": "mobile",
      "is_primary": true
    }
  ]
}

HTTP 200
[Captures]
etag: header "ETag"
[Asserts]
header "ETag" exists
jsonpath "$.id" == {{contact_id}}
jsonpath "$.first_name" == "John"
jsonpath "$.last_name" == "Doe"
jsonpath "$.nickname" == "JD"
jsonpath "$.organization" == "ACME Corp"
jsonpath "$.notes" == "Updated via Hurl test"


# ─────────────────────────────────────────────────────────────
# Step 9 – Delete John Doe (uses refreshed ETag from step 8)
# ─────────────────────────────────────────────────────────────
DELETE {{base_url}}/api/address-books/{{book_id}}/contacts/{{contact_id}}
Authorization: Bearer {{token}}
If-Match: {{etag}}

HTTP 204


# ─────────────────────────────────────────────────────────────
# Step 10 – Address book must be empty again
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/address-books/{{book_id}}/contacts
Authorization: Bearer {{token}}

HTTP 200
[Asserts]
jsonpath "$" count == 0


# ─────────────────────────────────────────────────────────────
# Step 11 – Delete the personal address book
# ─────────────────────────────────────────────────────────────
DELETE {{base_url}}/api/address-books/{{book_id}}
Authorization: Bearer {{token}}

HTTP 204


# ─────────────────────────────────────────────────────────────
# Step 12 – Verify the address book is gone from the list
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/address-books
Authorization: Bearer {{token}}

HTTP 200
[Asserts]
jsonpath "$[*].id" not contains {{book_id}}


# ─────────────────────────────────────────────────────────────
# Step 13 – List the system address book (OxiCloud users)
# ─────────────────────────────────────────────────────────────
GET {{base_url}}/api/address-books/system/contacts
Authorization: Bearer {{token}}

HTTP 200
[Asserts]
jsonpath "$" isCollection
