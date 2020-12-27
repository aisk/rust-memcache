#!/bin/bash
set -e

BASEDIR=$(dirname "$0")
MEMCACHED_VERSION="1.6.9"
MEMCACHED_TARBALL="memcached-$MEMCACHED_VERSION.tar.gz"
MEMCACHED_DIR="$BASEDIR/memcached-$MEMCACHED_VERSION"
MEMCACHED="$MEMCACHED_DIR/memcached"

SSL_KEY=$BASEDIR/assets/localhost.key
SSL_CERT=$BASEDIR/assets/localhost.crt
SSL_ROOT_CERT=$BASEDIR/assets/RUST_MEMCACHE_TEST_CERT.crt

echo "Building memcached $MEMCACHED_VERSION with TLS support"
if [[ ! -d "$MEMCACHED_DIR" ]]; then
    curl "https://memcached.org/files/$MEMCACHED_TARBALL" -O
    tar xvf "$MEMCACHED_TARBALL" -C "$BASEDIR"
    rm "$MEMCACHED_TARBALL"
fi

if [[ ! -f "$MEMCACHED" ]]; then
    pushd "$MEMCACHED_DIR"
    ./configure --enable-tls
    make
    popd
fi

echo "Starting memcached servers"
$MEMCACHED -V
$MEMCACHED -p 12345 -d
$MEMCACHED -p 12346 -d
$MEMCACHED -p 12347 -d
$MEMCACHED -p 12348 -d
$MEMCACHED -p 12349 -d
$MEMCACHED -p 12350 -d --enable-ssl -o "ssl_key=$SSL_KEY,ssl_chain_cert=$SSL_CERT"
$MEMCACHED -p 12351 -d --enable-ssl -o "ssl_key=$SSL_KEY,ssl_chain_cert=$SSL_CERT,ssl_verify_mode=2,ssl_ca_cert=$SSL_ROOT_CERT"
$MEMCACHED -U 22345 -d
$MEMCACHED -s /tmp/memcached.sock -d
$MEMCACHED -s /tmp/memcached2.sock -d
