#!/bin/sh
set -eu

secret_file="${FINELOR_SESSION_SECRET_FILE:-/app/.finelor/session.secret}"

if [ -z "${SESSION_SECRET:-}" ]; then
    mkdir -p "$(dirname "$secret_file")"

    if [ ! -s "$secret_file" ]; then
        umask 077
        head -c 96 /dev/urandom | base64 | tr -d '\n' > "$secret_file"
    fi

    SESSION_SECRET="$(cat "$secret_file")"
    export SESSION_SECRET
fi

exec "$@"
