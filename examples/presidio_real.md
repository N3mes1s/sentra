# Real Presidio Analyzer Integration (No Wrapper)

This guide shows how to run Sentra against the official Presidio Analyzer container directly, without any custom wrapper layer. The external HTTP plugin evaluates analyzer responses and blocks a request when **any PII entities are found** (non-empty result array/object at a JSON Pointer path).

## How It Works

1. Client calls Sentra API.
2. Sentra runs its plugin chain, invoking the external Presidio Analyzer.
3. Presidio returns a JSON array of recognitions (each with `entity_type`, `start`, `end`, `score`, etc.).
4. The external plugin uses a JSON Pointer to that array and, with `nonEmptyPointerBlocks=true`, blocks if the array is non-empty.
5. Sentra emits metrics, telemetry, and an allow/block decision.

## Analyzer Response Shape
The official analyzer returns an array at the top level, e.g.:
```json
[
  {
    "entity_type": "PHONE_NUMBER",
    "start": 42,
    "end": 54,
    "score": 0.87
  }
]
```
Because it is a top-level JSON array, the correct JSON Pointer to it is `/`.

## Plugin Config
`examples/presidio_real_plugin_config.json` (already present) uses pointer `/` with `nonEmptyPointerBlocks` set. Example excerpt:
```json
{
  "external_presidio": {
    "type": "externalHttp",
    "url": "http://presidio-analyzer:3000/analyze",
    "method": "POST",
    "timeoutMs": 2500,
    "failOpen": false,
    "blockField": "/",
    "nonEmptyPointerBlocks": true,
    "reasonCode": 860,
    "requestTemplate": {
      "text": "{{ plannerContext.userMessage }}",
      "language": "en"
    }
  }
}
```

Notes:
- `blockField` is `/` (root array).
- `nonEmptyPointerBlocks: true` means any non-empty array/object there triggers a block.
- `failOpen: false` ensures that timeouts or analyzer errors block by default (fail closed) for safety. Adjust if desired.

## Compose Setup
A ready-to-use compose file: `docker-compose.presidio.real.yml` spins up:
- `presidio-analyzer` (official image) on port 3000
- `sentra` on port 8080 using the real plugin config
- `real-sample-client` which sends a sample request after startup

## Run It
From repo root:
```
docker compose -f docker-compose.presidio.real.yml up --build
```
Watch the logs. After ~10s the sample client posts a request containing an email + SSN-like pattern. Presidio should detect entities (depending on model coverage) and Sentra should respond with a block decision (HTTP 200 payload with decision context). If the entity list is empty, the request will be allowed.

To send your own request once running:
```
curl -s -X POST 'http://localhost:8081/analyze-tool-execution?api-version=2025-05-01' \
  -H 'Authorization: Bearer demo' -H 'Content-Type: application/json' \
  -d '{"plannerContext":{"userMessage":"Email me at jane.doe@example.com and call 212-555-1234"},"toolDefinition":{"name":"SendEmail"},"inputValues":{"to":"jane.doe@example.com"}}' | jq
```

## Metrics & Telemetry
Prometheus metrics at: `http://localhost:8081/metrics`
Look for labels like `plugin="external_presidio"` and per-plugin histograms.

Telemetry file (host): `./telemetry_real.log`
Each line contains JSON with plugin timing and decision rationale.

## Adjusting Sensitivity
You can add Presidio recognizers or modify its configuration via environment variables and volumes—refer to Presidio docs. Sentra just treats any non-empty array at `/` as PII presence.

## Troubleshooting
- If `sentra` starts before analyzer is healthy: the first few external calls may timeout; with `failOpen: false` they will block. Wait for analyzer health, retry.
- Ensure Docker has enough memory for spaCy models (2GB+ recommended).
- If you want allow-on-error semantics, set `failOpen: true` in the plugin config.

## Next Ideas
- Chain multiple external plugins (e.g., Presidio + custom secrets service) — order defines short-circuiting.
- Add per-entity filtering by adapting a tiny transform service (or future built-in filtering logic) if you only want to block on certain entity types.

---
This real integration removes the wrapper layer and demonstrates the external guardrail pipeline with a production-grade PII detector.
