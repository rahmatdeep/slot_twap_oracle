# Slot TWAP Oracle

A Solana program that computes slot-weighted time-weighted average prices (TWAP) for arbitrary trading pairs. Built with [Anchor](https://www.anchor-lang.com/).

## How It Works

The oracle tracks price using slot-weighted cumulative pricing. On every `update_price` call:

```
cumulative_price += last_price * (current_slot - last_slot)
```

The TWAP (called SWAP — Slot-Weighted Average Price) over any window is:

```
SWAP = (cumulative_now - cumulative_past) / (slot_now - slot_past)
```

Slots are used instead of timestamps because Solana's `Clock::unix_timestamp` is a stake-weighted median that can drift or stall. Slots increment deterministically and are immune to validator manipulation.

## Architecture

```
programs/slot_twap_oracle/src/
├── instructions/
│   ├── initialize_oracle.rs   # Create oracle + observation buffer for a pair
│   ├── update_price.rs        # Update price, accumulate cumulative, store observation
│   └── get_swap.rs            # Read-only SWAP computation over a slot window
├── state/
│   ├── oracle.rs              # Oracle account (price, cumulative, slot, bump)
│   └── observation.rs         # ObservationBuffer ring buffer
├── math/
│   └── swap.rs                # Off-chain compute_swap utility
├── utils/
│   └── ring_buffer.rs         # push_observation, get_observation_before_slot
├── errors/mod.rs
├── events.rs                  # PriceUpdated event
└── lib.rs
```

### Accounts

Each trading pair gets two PDAs with no shared global state:

- **Oracle** — seeded by `["oracle", base_mint, quote_mint]`
- **ObservationBuffer** — seeded by `["observation", oracle]`, fixed-capacity ring buffer

This design is optimized for Solana's Sealevel runtime: updates to different pairs touch completely disjoint account sets, enabling parallel execution.

### Instructions

| Instruction | Description |
|---|---|
| `initialize_oracle(base_mint, quote_mint, capacity)` | Creates oracle and observation buffer PDAs |
| `update_price(new_price)` | Accumulates slot-weighted price, stores observation |
| `get_swap(window_slots)` | Returns SWAP over the last N slots (read-only) |

## Project Structure

```
slot_twap_oracle/
├── programs/slot_twap_oracle/  # Anchor program
├── tests/                      # Rust integration tests (litesvm)
├── sdk/                        # TypeScript SDK
├── bots/updater/               # Price updater bot (Raydium)
└── migrations/                 # Anchor deployment script
```

## SDK

The TypeScript SDK provides a high-level client:

```typescript
import { SlotTwapOracleClient } from "@slot-twap-oracle/sdk";

const client = new SlotTwapOracleClient(provider);

// Initialize a new pair
await client.initializeOracle(baseMint, quoteMint, 64, payer);

// Update price
await client.updatePrice(oraclePda, new BN(price), payer);

// Fetch SWAP on-chain
const swap = await client.getSwap(oraclePda, new BN(100));

// Compute SWAP off-chain from observation history
const swap = await client.computeSwapFromChain(baseMint, quoteMint, 100);
```

## Updater Bot

The bot in `bots/updater/` fetches prices from a Raydium AMM pool and calls `update_price` every 30 seconds.

```bash
cd bots/updater
cp .env.example .env  # fill in RPC_URL, KEYPAIR_PATH, RAYDIUM_AMM_ID, mints
npm install
npm start
```

## Testing

All tests use [litesvm](https://github.com/LiteSVM/litesvm) for fast in-process execution:

```bash
# Build the program first
anchor build

# Run all tests
cargo test -p slot_twap_oracle_tests
```

36 tests covering:

- Happy-path: init, update, cumulative math, SWAP computation
- Observation buffer: ring wrap, capacity-1, slot lookups after overwrite
- Edge cases: stale slot, zero price, large values, single slot delta, double init
- Sealevel parallelism: batched multi-pair tx, same-slot independent txs

## Program ID

```
7LKj9Yk62ddRjtTHvvV6fmquD9h7XbcvKKa7yGtocdsT
```

## License

ISC
