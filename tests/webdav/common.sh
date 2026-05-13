#!/bin/bash

source test.env

err() {
    echo "$*" >&2
}

warn() {
    echo "$*" >&2
}

success() {
    echo "ok";
}

# Create the first admin account if the server is freshly initialised.
# Silently succeeds when the account already exists (403 = already set up).
oxicloud_setup() {
    SETUP_DATA='{"username":"'$username'","email":"'$email'","password":"'$password'"}'
    SETUP_STATUS=$(curl -s -o /dev/null -w "%{http_code}" \
        -X POST -H "Content-Type: application/json" \
        -d "$SETUP_DATA" "$base_url/api/setup")

    case "$SETUP_STATUS" in
        201) echo "setup: admin account created" ;;
        403) ;;  # already initialised — normal on second run
        *)   err "setup: unexpected status $SETUP_STATUS"; exit 1 ;;
    esac
}

# returns TOKEN variable
oxicloud_login() {

    oxicloud_setup

    LOGIN_DATA='{"username":"'$username'","password":"'$password'"}'

    LOGIN_RESPONSE=$(curl -s -X POST -H "Content-Type: application/json" -d "$LOGIN_DATA" $base_url/api/auth/login)

    echo $?
    jq -r '.error' <<<$LOGIN_RESPONSE

    if [[ "$(jq -r '.error' <<<$LOGIN_RESPONSE)" != "null" ]] 
    then
        err "Login Error: $LOGIN_RESPONSE"
        exit 1
    fi

    TOKEN=$(jq -r '.access_token' <<<$LOGIN_RESPONSE)

    if [[ -z "$TOKEN" || "$TOKEN" == "null" ]]
    then
        echo access_token missing in response: $LOGIN_RESPONSE
        exit 1
    fi

    echo "login successful, Got JWT token containing informations:" $(cut -d . -f 2 <<<$TOKEN | base64 -d)
}

# remove trailing /
base_url="${base_url%/}"

echo starting $0 tests on server $base_url 
