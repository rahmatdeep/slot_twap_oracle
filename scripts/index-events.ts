#!/usr/bin/env npx tsx
/**
 * Streams OracleUpdate events via Geyser gRPC and stores them in PostgreSQL.
 *
 * Usage:
 *   GEYSER_ENDPOINT=http://localhost:10000 \
 *   POSTGRES_URL=postgres://user:pass@localhost:5432/oracle \
 *   npx tsx scripts/index-events.ts
 *
 * Environment:
 *   GEYSER_ENDPOINT    - Yellowstone gRPC endpoint (required)
 *   GEYSER_X_TOKEN     - Auth token for the gRPC endpoint (optional)
 *   POSTGRES_URL       - PostgreSQL connection string (required)
 *
 * Requires: @triton-one/yellowstone-grpc, pg
 */

import Client, {
  CommitmentLevel,
  SubscribeUpdate,
  SubscribeRequest,
} from "@triton-one/yellowstone-grpc";
import { Pool } from "pg";
import { PublicKey } from "@solana/web3.js";
import bs58 from "bs58";
import { SlotTwapOracleClient, PROGRAM_ID } from "@slot-twap-oracle/sdk";

// ── Config ──

const GEYSER_ENDPOINT = process.env.GEYSER_ENDPOINT;
const GEYSER_X_TOKEN = process.env.GEYSER_X_TOKEN;
const POSTGRES_URL = process.env.POSTGRES_URL;

if (!GEYSER_ENDPOINT) {
  console.error("Error: GEYSER_ENDPOINT is required");
  process.exit(1);
}
if (!POSTGRES_URL) {
  console.error("Error: POSTGRES_URL is required");
  process.exit(1);
}

// ── Database ──

async function initDb(pool: Pool): Promise<void> {
  await pool.query(`
    CREATE TABLE IF NOT EXISTS oracle_updates (
      id               BIGSERIAL PRIMARY KEY,
      tx_signature     TEXT NOT NULL,
      oracle_pubkey    TEXT NOT NULL,
      price            NUMERIC NOT NULL,
      cumulative_price NUMERIC NOT NULL,
      slot             BIGINT NOT NULL,
      updater          TEXT NOT NULL,
      indexed_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
      UNIQUE (tx_signature, oracle_pubkey)
    )
  `);
  await pool.query(
    `CREATE INDEX IF NOT EXISTS idx_oracle_updates_oracle ON oracle_updates (oracle_pubkey)`
  );
  await pool.query(
    `CREATE INDEX IF NOT EXISTS idx_oracle_updates_slot ON oracle_updates (slot DESC)`
  );
}

// ── Stream handler ──

async function processUpdate(pool: Pool, update: SubscribeUpdate): Promise<void> {
  const txUpdate = update.transaction;
  if (!txUpdate?.transaction) return;

  const txInfo = txUpdate.transaction;
  if (!txInfo.meta || txInfo.meta.logMessagesNone) return;

  const logs = txInfo.meta.logMessages;
  const events = SlotTwapOracleClient.decodeOracleUpdateLogs(logs);
  if (events.length === 0) return;

  const signature = bs58.encode(txInfo.signature);
  const slot = BigInt(txUpdate.slot);

  for (const event of events) {
    try {
      await pool.query(
        `INSERT INTO oracle_updates (tx_signature, oracle_pubkey, price, cumulative_price, slot, updater)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (tx_signature, oracle_pubkey) DO NOTHING`,
        [
          signature,
          event.oracle.toBase58(),
          event.price.toString(),
          event.cumulativePrice.toString(),
          slot.toString(),
          event.updater.toBase58(),
        ]
      );
    } catch (err) {
      console.error(
        `${ts()} [error] Failed to insert: ${(err as Error).message}`
      );
    }
  }

  console.log(
    `${ts()} [indexed] slot=${slot} tx=${signature.slice(0, 16)}... events=${events.length}`
  );
}

function ts(): string {
  return new Date().toISOString();
}

// ── Main ──

async function run(): Promise<void> {
  const pool = new Pool({ connectionString: POSTGRES_URL });

  console.log(`${ts()} [init] Geyser:   ${GEYSER_ENDPOINT}`);
  console.log(`${ts()} [init] Postgres: ${POSTGRES_URL!.replace(/\/\/.*@/, "//***@")}`);
  console.log(`${ts()} [init] Program:  ${PROGRAM_ID.toBase58()}`);

  await initDb(pool);
  console.log(`${ts()} [init] Database ready`);

  const client = new Client(GEYSER_ENDPOINT!, GEYSER_X_TOKEN, undefined);
  await client.connect();
  const stream = await client.subscribe();

  // Subscribe to transactions that mention our program
  const request: SubscribeRequest = {
    accounts: {},
    slots: {},
    transactions: {
      oracle: {
        vote: false,
        failed: false,
        signature: undefined,
        accountInclude: [PROGRAM_ID.toBase58()],
        accountExclude: [],
        accountRequired: [],
      },
    },
    transactionsStatus: {},
    blocks: {},
    blocksMeta: {},
    entry: {},
    accountsDataSlice: [],
    commitment: CommitmentLevel.CONFIRMED,
    ping: undefined,
  };

  // Send subscription request
  stream.write(request);
  console.log(`${ts()} [stream] Subscribed to program transactions`);

  let eventCount = 0;

  stream.on("data", async (update: SubscribeUpdate) => {
    if (update.transaction) {
      await processUpdate(pool, update);
      eventCount++;
    }
  });

  stream.on("error", (err: Error) => {
    console.error(`${ts()} [stream] Error: ${err.message}`);
  });

  stream.on("end", () => {
    console.log(`${ts()} [stream] Stream ended. Indexed ${eventCount} transaction(s) total.`);
  });

  // Graceful shutdown
  const shutdown = async () => {
    console.log(`\n${ts()} [shutdown] Closing...`);
    stream.destroy();
    await pool.end();
    console.log(`${ts()} [shutdown] Done. Indexed ${eventCount} transaction(s).`);
    process.exit(0);
  };

  process.on("SIGINT", shutdown);
  process.on("SIGTERM", shutdown);
}

run().catch((err) => {
  console.error(err);
  process.exit(1);
});
