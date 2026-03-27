# Updater Bot — Operations Runbook

## Overview

The updater bot fetches prices from DEX pools (Raydium, Orca, Meteora), computes a median, and submits on-chain `update_price` transactions for each configured oracle pair. It runs as a long-lived Node.js process.

## Prerequisites

- Node.js 20+
- Funded Solana keypair (pays tx fees)
- RPC endpoint (mainnet or devnet)
- `config/pairs.json` configured with oracle addresses and pool sources
- SDK linked: `cd sdk && npm run build && npm link && cd ../bots/updater && npm link @slot-twap-oracle/sdk`

## Starting

```bash
cd bots/updater
cp .env.example .env
# Edit .env with RPC_URL, KEYPAIR_PATH, ORACLE_PROGRAM_ID
cp config/pairs.example.json config/pairs.json
# Edit config/pairs.json with real oracle and pool addresses
npm start
```

## Environment Variables

| Variable | Required | Default | Description |
|---|---|---|---|
| `RPC_URL` | Yes | — | Solana RPC endpoint |
| `KEYPAIR_PATH` | Yes | — | Path to payer keypair JSON |
| `ORACLE_PROGRAM_ID` | Yes | — | Oracle program pubkey |
| `PAIRS_CONFIG_PATH` | No | `config/pairs.json` | Path to pairs config |
| `MIN_SOURCES` | No | `2` | Minimum valid price sources per pair |
| `UPDATE_INTERVAL_MS` | No | `30000` | Tick interval (ms) |
| `METRICS_PATH` | No | `data/metrics.json` | Persistent metrics file |
| `PROMETHEUS_PORT` | No | `9090` | Prometheus metrics port |
| `TELEGRAM_BOT_TOKEN` | No | — | Telegram bot token for alerts |
| `TELEGRAM_CHAT_ID` | No | — | Telegram chat ID for alerts |

## Monitoring

### Prometheus Metrics

Available at `http://localhost:9090/metrics`:

| Metric | Type | Description |
|---|---|---|
| `updates_successful_total` | counter | Total successful price updates |
| `updates_failed_total` | counter | Total failed updates (tx errors) |
| `updates_skipped_total` | counter | Skipped (insufficient sources, deviation, paused) |
| `stale_oracles_total` | counter | Stale oracle detections |
| `oracle_last_update_slot{pair}` | gauge | Last successful update slot per pair |

### Grafana

Import `grafana/dashboard.json` into Grafana. Requires a Prometheus datasource scraping the bot's `/metrics` endpoint.

Panels:
- Update rate per minute (success/fail/skip)
- Total counters with red threshold on failures
- Per-oracle slot tracking
- Staleness alert bar chart
- Oracle slot lag with 50/100 threshold lines

### Console Logs

All logs include ISO timestamps and pair names:

```
2026-03-28T12:00:00.000Z [SOL/USDC] Fetching prices...
2026-03-28T12:00:00.100Z [SOL/USDC]   raydium: 134.52
2026-03-28T12:00:00.200Z [SOL/USDC]   orca: 134.48
2026-03-28T12:00:00.300Z [SOL/USDC]   meteora: FAILED - Meteora LB pair not found
2026-03-28T12:00:00.400Z [SOL/USDC] Median: 134.5 (2/3 sources after filter) -> scaled: 134500000000
2026-03-28T12:00:01.000Z [SOL/USDC] update_price tx: 4xK... (slot=285000000, price=134.5)
```

### Persistent Metrics

Stored in `data/metrics.json`, survives restarts:

```json
{
  "successful": 1420,
  "failed": 3,
  "skipped": 12,
  "staleOracles": 1,
  "lastSuccessfulUpdate": "2026-03-28T12:00:01.000Z",
  "lastUpdateSlot": {
    "SOL/USDC": 285000000,
    "ETH/USDC": 284999998
  }
}
```

## Alerting

### Telegram

Set `TELEGRAM_BOT_TOKEN` and `TELEGRAM_CHAT_ID` in `.env`. Alerts fire when any oracle hasn't been updated in >100 slots. Rate-limited to once per 5 minutes.

Example alert:
```
🚨 Oracle Staleness Alert
2 oracle(s) stale:
  • SOL/USDC: last slot 285000000 (150 behind)
  • ETH/USDC: last slot 284999800 (350 behind)
```

**Setup:**
1. Create a bot via [@BotFather](https://t.me/BotFather), copy the token
2. Send a message to your bot, then get your chat ID from `https://api.telegram.org/bot<TOKEN>/getUpdates`
3. Set both in `.env`

## Troubleshooting

### Bot won't start

| Symptom | Cause | Fix |
|---|---|---|
| `Missing env var: RPC_URL` | `.env` not configured | Copy `.env.example` and fill values |
| `Pairs config not found` | No `config/pairs.json` | Copy `config/pairs.example.json` |
| `Needs >= 2 sources` | Pair has fewer sources than `MIN_SOURCES` | Add more pool addresses or lower `MIN_SOURCES` |
| `Keypair file not found` | Bad `KEYPAIR_PATH` | Check path, supports `~` expansion |

### Updates failing

| Symptom | Cause | Fix |
|---|---|---|
| `StaleSlot` | Slot hasn't advanced between ticks | Increase `UPDATE_INTERVAL_MS` |
| `PriceDeviationTooLarge` | Price jumped >10% (or per-oracle threshold) | Check source pool health; owner can widen via `set_max_deviation` |
| `OraclePaused` | Oracle is paused | Owner must call `set_paused(false)` |
| `Insufficient funds` | Payer wallet is empty | Top up the keypair |

### Updates skipping

| Log message | Cause | Fix |
|---|---|---|
| `All sources failed` | Every pool fetch errored | Check RPC, verify pool addresses are valid |
| `Only N/M sources` | Too few sources succeeded | Check which source failed in logs, verify pool |
| `High source deviation` | Sources disagree by >5% | One pool may have stale/manipulated liquidity |
| `X rejected: deviation Y%` | Individual source is an outlier | Pool may be imbalanced, check on-chain |

### Pool mint mismatch

```
Raydium: pool mints (ABC, DEF) do not match oracle mints (GHI, JKL)
```

The pool's token pair doesn't match the oracle's base/quote mints. Verify the pool address in `pairs.json` is for the correct trading pair. The bot auto-inverts if mints are in reverse order.

## Shutdown

The bot handles `SIGINT` (Ctrl+C) and `SIGTERM` gracefully:
1. Stops all timers
2. Closes Prometheus server
3. Flushes metrics to disk
4. Logs final summary

```
2026-03-28T12:30:00.000Z [updater] Received SIGTERM, shutting down...
2026-03-28T12:30:00.001Z [metrics] successful=1420 failed=3 skipped=12 stale=1 last_success=2026-03-28T12:29:30.000Z
2026-03-28T12:30:00.002Z [updater] Shutdown complete.
```

## Health Checks

For process managers (systemd, Docker, k8s):

- **Liveness**: `curl http://localhost:9090/metrics` returns 200
- **Readiness**: `updates_successful_total` counter is incrementing
- **Staleness**: `stale_oracles_total` should not increase continuously

### Systemd Unit

```ini
[Unit]
Description=Slot TWAP Oracle Updater Bot
After=network.target

[Service]
Type=simple
User=oracle
WorkingDirectory=/opt/slot-twap-oracle/bots/updater
ExecStart=/usr/bin/node dist/index.js
Restart=on-failure
RestartSec=10
EnvironmentFile=/opt/slot-twap-oracle/bots/updater/.env

[Install]
WantedBy=multi-user.target
```

### Docker

```dockerfile
FROM node:20-slim
WORKDIR /app
COPY bots/updater/ .
RUN npm install
CMD ["node", "dist/index.js"]
```

## Pair Configuration

`config/pairs.json`:

```json
[
  {
    "name": "SOL/USDC",
    "oracle": "ORACLE_PDA_PUBKEY",
    "baseMint": "So11111111111111111111111111111111111111112",
    "quoteMint": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
    "sources": {
      "raydium": "RAYDIUM_POOL_ADDRESS",
      "orca": "ORCA_WHIRLPOOL_ADDRESS",
      "meteora": "METEORA_DLMM_ADDRESS"
    }
  }
]
```

- Each pair must have `name`, `oracle`, `baseMint`, `quoteMint`, and `sources`
- At least `MIN_SOURCES` sources must be configured per pair
- Sources are optional individually — omit a key to skip that DEX
- The bot validates pool mints match oracle mints at fetch time
