#!/usr/bin/env bash
# Query local proxy at 127.0.0.1:31415 for available models.
# Output goes to the .log file next to this script.

set -euo pipefail

BASE_URL="http://127.0.0.1:31415/v1"
TOKEN="freellmapi-d928a05662409b1611e696db563c0837b920044f9a576478"
LOG="/Users/HawkSept/myproject/myapp/localquark-rust/scripts/_pending/list-models-31415-20260703-000000.log"

mkdir -p "$(dirname "$LOG")"

{
  echo "=== Probe 1: GET ${BASE_URL}/models (Anthropic-style header) ==="
  curl -sS -w "\n--- HTTP %{http_code} in %{time_total}s ---\n" \
    -H "x-api-key: ${TOKEN}" \
    -H "anthropic-version: 2023-06-01" \
    "${BASE_URL}/models"

  echo ""
  echo "=== Probe 2: GET ${BASE_URL}/models (Bearer header) ==="
  curl -sS -w "\n--- HTTP %{http_code} in %{time_total}s ---\n" \
    -H "Authorization: Bearer ${TOKEN}" \
    "${BASE_URL}/models"

  echo ""
  echo "=== Probe 3: GET ${BASE_URL}/models (no auth) ==="
  curl -sS -w "\n--- HTTP %{http_code} in %{time_total}s ---\n" \
    "${BASE_URL}/models"
} | tee "$LOG"

echo ""
echo "Wrote: $LOG"