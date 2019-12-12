#!/bin/bash
set -e

SSL_KEY=tests/assets/localhost.key
SSL_CERT=tests/assets/localhost.crt
SSL_ROOT_CERT=tests/assets/RUST_MEMCACHE_TEST_CERT.crt

memcached -p 12345 -d
memcached -p 12346 -d
memcached -p 12347 -d
memcached -p 12348 -d
memcached -p 12349 -d
memcached -p 12350 -d --enable-ssl -o "ssl_key=$SSL_KEY,ssl_chain_cert=$SSL_CERT"
memcached -p 12351 -d --enable-ssl -o "ssl_key=$SSL_KEY,ssl_chain_cert=$SSL_CERT,ssl_verify_mode=2,ssl_ca_cert=$SSL_ROOT_CERT"
memcached -U 22345 -d
memcached -s /tmp/memcached.sock -d
memcached -s /tmp/memcached2.sock -d
