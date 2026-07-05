#!/bin/bash
# Generate self-signed TLS certificate for localhost if not present
CERT_DIR="$1"
CERT="$CERT_DIR/localhost.pem"
KEY="$CERT_DIR/localhost-key.pem"

if [[ -f "$CERT" && -f "$KEY" ]]; then
    exit 0
fi

mkdir -p "$CERT_DIR"
openssl req -x509 -newkey rsa:2048 -keyout "$KEY" -out "$CERT" \
    -days 3650 -nodes -subj "/CN=localhost" \
    -addext "subjectAltName=DNS:localhost,IP:127.0.0.1" 2>/dev/null

if [[ $? -eq 0 ]]; then
    echo "Generated self-signed TLS cert: $CERT"
else
    echo "Failed to generate TLS cert" >&2
    exit 1
fi
