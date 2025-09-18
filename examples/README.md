# Examples Overview

Organized by category for clarity:

## Policy / Built-in Plugins
- `policy_config_example.json` – Sample policy pack configuration.
- `load_test.rs` – Simple concurrency/load generator hitting the analyze endpoint.

## External HTTP Integration
- `external_http_example.json` – Demonstrates defining an external HTTP plugin.


## Presidio (External Analyzer)
Located under `presidio/`:
- `presidio_real_plugin_config.json` – External plugin config using root pointer + structural non-empty block.
- `presidio_real.md` – Guide / walkthrough.
- Supporting stack: root `docker-compose.presidio.real.yml` (kept at repo root for discoverability; references the config via relative path) and script `scripts/run_real_presidio_e2e.sh`.

## Running the Presidio E2E
```bash
./scripts/run_real_presidio_e2e.sh
```
Requires Docker; ensures at least one block and surfaces metrics.

## Notes
* Root compose file intentionally retained at top-level to simplify `docker compose -f docker-compose.presidio.real.yml up` and keep CI/CD options open.
* Example configs avoid leaking sensitive data. Adjust endpoints/tokens for your environment.
