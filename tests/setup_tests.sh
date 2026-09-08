#!/bin/bash
# Starts the memcached instances the integration tests expect.
#
# By default this runs the memcached binary on PATH, which must be built with
# TLS support (the Debian, Ubuntu and Arch packages qualify) and be 1.6.40 or
# newer, the release that added the conditional meta get used by unless_cas.
# Set MEMCACHED to point at another binary, or set MEMCACHED_IMAGE to run each
# instance from the official Docker image on the host network instead, which
# is what CI does because the Ubuntu package is too old.
set -e

ASSETS=$(realpath "$(dirname "$0")/assets")
MEMCACHED="${MEMCACHED:-memcached}"

SSL_KEY=$ASSETS/localhost.key
SSL_CERT=$ASSETS/localhost.crt
SSL_ROOT_CERT=$ASSETS/RUST_MEMCACHE_TEST_CERT.crt

start() {
    if [[ -n "$MEMCACHED_IMAGE" ]]; then
        docker run -d --rm --network host -v /tmp:/tmp -v "$ASSETS:$ASSETS:ro" \
            "$MEMCACHED_IMAGE" "$@" > /dev/null
    else
        "$MEMCACHED" "$@" -d
    fi
}

if [[ -n "$MEMCACHED_IMAGE" ]]; then
    docker run --rm "$MEMCACHED_IMAGE" -V
else
    if ! "$MEMCACHED" -h 2>&1 | grep -q -- '--enable-ssl'; then
        echo "error: $MEMCACHED was built without TLS support" >&2
        exit 1
    fi
    "$MEMCACHED" -V
fi

echo "Starting memcached servers"
start -p 12345
start -p 12346
start -p 12347
start -p 12348
start -p 12349
start -p 12350 --enable-ssl -o "ssl_key=$SSL_KEY,ssl_chain_cert=$SSL_CERT"
start -p 12351 --enable-ssl -o "ssl_key=$SSL_KEY,ssl_chain_cert=$SSL_CERT,ssl_verify_mode=2,ssl_ca_cert=$SSL_ROOT_CERT"
start -U 22345
start -s /tmp/memcached.sock -a 777
start -s /tmp/memcached2.sock -a 777
