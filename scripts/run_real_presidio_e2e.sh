#!/usr/bin/env bash
set -euo pipefail

COMPOSE_FILE="docker-compose.presidio.real.yml"
BLOCK_PAYLOAD='{"plannerContext":{"userMessage":"Email john.smith@example.com and SSN 123-45-6789"},"toolDefinition":{"name":"SendEmail"},"inputValues":{"to":"john.smith@example.com"}}'
ALLOW_PAYLOAD='{"plannerContext":{"userMessage":"Generate a project roadmap summary"},"toolDefinition":{"name":"SummaryTool"},"inputValues":{}}'

color() { printf "\033[%sm%s\033[0m" "$1" "$2"; }
info() { echo "$(color 36 [INFO]) $*"; }
ok() { echo "$(color 32 [ OK ]) $*"; }
warn() { echo "$(color 33 [WARN]) $*"; }
err() { echo "$(color 31 [FAIL]) $*"; }

info "Bringing up stack ($COMPOSE_FILE)"
docker compose -f "$COMPOSE_FILE" up -d --build

info "Waiting for Presidio analyzer health..."
for i in {1..40}; do
  if curl -sf http://localhost:3000/health >/dev/null; then ok "Analyzer healthy"; break; fi
  sleep 1
  [[ $i == 40 ]] && { err "Analyzer never became healthy"; exit 1; }
done

info "Waiting for Sentra health..."
for i in {1..40}; do
  if curl -sf http://localhost:8081/healthz >/dev/null; then ok "Sentra healthy"; break; fi
  sleep 1
  [[ $i == 40 ]] && { err "Sentra never became healthy"; exit 1; }
done

info "Sending blocking request"
BLOCK_RESP=$(curl -s -X POST 'http://localhost:8081/analyze-tool-execution?api-version=2025-05-01' \
  -H 'Authorization: Bearer demo' -H 'Content-Type: application/json' -d "$BLOCK_PAYLOAD")
echo "$BLOCK_RESP" | jq . 2>/dev/null || echo "$BLOCK_RESP"

if [[ $(echo "$BLOCK_RESP" | grep -c '"blockAction":true') -eq 0 ]]; then
  err "Expected blockAction true for blocking payload"; exit 1; fi
if [[ $(echo "$BLOCK_RESP" | grep -c '"reasonCode":860') -eq 0 ]]; then
  warn "reasonCode 860 not found (continuing)"; fi

info "Sending allowing request"
ALLOW_RESP=$(curl -s -X POST 'http://localhost:8081/analyze-tool-execution?api-version=2025-05-01' \
  -H 'Authorization: Bearer demo' -H 'Content-Type: application/json' -d "$ALLOW_PAYLOAD")
echo "$ALLOW_RESP" | jq . 2>/dev/null || echo "$ALLOW_RESP"

if [[ $(echo "$ALLOW_RESP" | grep -c '"blockAction":false') -eq 0 ]]; then
  err "Expected blockAction false for allow payload"; exit 1; fi

info "Fetching metrics snapshot"
METRICS=$(curl -s http://localhost:8081/metrics)
REQS=$(echo "$METRICS" | awk '/^sentra_requests_total /{print $2}')
BLOCKS=$(echo "$METRICS" | awk '/^sentra_blocks_total /{print $2}')
PLUGIN_EVAL=$(echo "$METRICS" | awk '/sentra_plugin_eval_ms_count\{plugin="external_presidio"}/{print $2}')
PLUGIN_BLOCKS=$(echo "$METRICS" | awk '/sentra_plugin_blocks_total\{plugin="external_presidio"}/{print $2}')

info "Metrics: requests=$REQS blocks=$BLOCKS plugin_eval_count=$PLUGIN_EVAL plugin_blocks=$PLUGIN_BLOCKS"

FAIL=0
[[ "$REQS" -lt 2 ]] && { warn "Expected at least 2 requests"; FAIL=1; }
[[ "$BLOCKS" -lt 1 ]] && { warn "Expected at least 1 block"; FAIL=1; }
[[ "$PLUGIN_EVAL" -lt 2 ]] && { warn "Expected external_presidio eval count >=2"; FAIL=1; }
[[ "$PLUGIN_BLOCKS" -lt 1 ]] && { warn "Expected external_presidio blocks >=1"; FAIL=1; }

if [[ $FAIL -eq 0 ]]; then
  ok "E2E test passed"
else
  err "E2E test completed with warnings/failures"
  exit 2
fi

info "Telemetry tail (if available)"
if [[ -d telemetry_data ]]; then
  ls -l telemetry_data || true
  tail -n 20 telemetry_data/telemetry.log 2>/dev/null || true
fi
