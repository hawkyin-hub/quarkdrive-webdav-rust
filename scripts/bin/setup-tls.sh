#!/usr/bin/env bash
# Stage 12b.2: ensure cert.pem + key.pem exist in $LOCALQUARK_CERT_DIR
# and (on macOS) are trusted in the System keychain so webdavfs_agent
# accepts the self-signed cert.
#
# Idempotent: if both files are present and the cert is already trusted
# (or we're not on Darwin), the script is a no-op. Safe to call from
# run-localquark.sh on every launch.

set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LOCALQUARK_REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
# shellcheck source=lib-common.sh
source "$SCRIPT_DIR/lib-common.sh"

ensure_app_dir

CERT_FILE="$LOCALQUARK_CERT_DIR/cert.pem"
KEY_FILE="$LOCALQUARK_CERT_DIR/key.pem"

if [ -f "$CERT_FILE" ] && [ -f "$KEY_FILE" ]; then
    log "tls: cert+key already present, skipping generation"
else
    log "tls: generating self-signed cert in $LOCALQUARK_CERT_DIR"
    # 825d validity matches mkcert defaults; CN=localhost + SAN covers
    # both DNS and IP forms that webdavfs_agent and curl will probe.
    /usr/bin/openssl req -x509 -newkey rsa:2048 -nodes \
        -keyout "$KEY_FILE" \
        -out "$CERT_FILE" \
        -days 825 \
        -subj "/CN=127.0.0.1" \
        -addext "subjectAltName=DNS:localhost,IP:127.0.0.1" \
        -addext "extendedKeyUsage=serverAuth" \
        -addext "keyUsage=digitalSignature,keyEncipherment" \
        2>>"$LOCALQUARK_LOG_FILE"
    chmod 0600 "$KEY_FILE"
    chmod 0644 "$CERT_FILE"
    log "tls: cert generated ($(wc -c < "$CERT_FILE") bytes, $(wc -c < "$KEY_FILE") bytes key)"
fi

# Trust the cert on macOS so webdavfs_agent (and curl without -k) can
# verify the local server. On other platforms this is a no-op.
if [ "$(uname -s)" = "Darwin" ]; then
    if /usr/bin/security verify-cert -c "$CERT_FILE" >/dev/null 2>&1; then
        log "tls: cert already in System keychain, skipping add-trusted-cert"
    else
        log "tls: trusting cert in System keychain (sudo prompt expected)"
        # Non-fatal: in headless / sandbox / non-interactive sessions
        # `security add-trusted-cert` to the System keychain requires a
        # sudo prompt that we cannot satisfy. The cert is still on disk
        # and reachable via --tls-cert-dir; webdavfs_agent will simply
        # need an extra manual `security add-trusted-cert` step on
        # operator-owned machines. Log + carry on.
        if /usr/bin/security add-trusted-cert -d -r trustRoot \
                -k /Library/Keychains/System.keychain "$CERT_FILE" \
                2>>"$LOCALQUARK_LOG_FILE"; then
            log "tls: cert trusted in System keychain"
        else
            log "WARN: add-trusted-cert failed (rc=$?); see $LOCALQUARK_LOG_FILE"
        fi
    fi
else
    log "tls: non-Darwin host, skipping keychain trust step"
fi
