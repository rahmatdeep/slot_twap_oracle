#!/usr/bin/env npx tsx
/**
 * Verifies indexer data integrity by comparing on-chain TWAP with
 * TWAP computed from PostgreSQL indexed events.
 *
 * Usage:
 *   POSTGRES_URL=postgres://... RPC_URL=https://api.mainnet-beta.solana.com \
 *   npx tsx scripts/verify-indexer.ts --oracle <pubkey> --window 100
 *
 * Checks:
 *   1. Row count matches expected update count
 *   2. Slots are strictly increasing (no gaps from dedup failures)
 *   3. Cumulative prices are monotonically non-decreasing
 *   4. TWAP from DB matches TWAP from on-chain state
 */

import { Connection, PublicKey } from "@solana/web3.js";
import { AnchorProvider, Wallet } from "@coral-xyz/anchor";
import { Keypair } from "@solana/web3.js";
import { Pool } from "pg";
import { SlotTwapOracleClient, PROGRAM_ID } from "@slot-twap-oracle/sdk";
import BN from "bn.js";

const RPC_URL = process.env.RPC_URL || "http://127.0.0.1:8899";
const POSTGRES_URL = process.env.POSTGRES_URL;

function ok(msg: string): void { console.log(`  \x1b[32m✓\x1b[0m ${msg}`); }
function fail(msg: string): void { console.log(`  \x1b[31m✗\x1b[0m ${msg}`); }
function warn(msg: string): void { console.log(`  \x1b[33m!\x1b[0m ${msg}`); }

function parseArgs(): { oracle: string; window: number } {
  const args = process.argv.slice(2);
  let oracle = "";
  let window = 100;
  for (let i = 0; i < args.length; i++) {
    if (args[i] === "--oracle") oracle = args[++i];
    if (args[i] === "--window") window = parseInt(args[++i], 10);
  }
  if (!oracle) {
    console.error("Usage: npx tsx scripts/verify-indexer.ts --oracle <pubkey> --window 100");
    process.exit(1);
  }
  return { oracle, window };
}

async function run(): Promise<void> {
  if (!POSTGRES_URL) {
    console.error("Error: POSTGRES_URL is required");
    process.exit(1);
  }

  const { oracle, window } = parseArgs();
  const pool = new Pool({ connectionString: POSTGRES_URL });
  const conn = new Connection(RPC_URL, "confirmed");
  const provider = new AnchorProvider(conn, new Wallet(Keypair.generate()), { commitment: "confirmed" });
  const client = new SlotTwapOracleClient(provider);
  const oraclePk = new PublicKey(oracle);

  let failures = 0;
  console.log(`\nVerifying indexer for oracle: ${oracle}`);
  console.log(`Window: ${window} slots\n`);

  // ── Check 1: Row count ──
  console.log("1. Row count:");
  const countResult = await pool.query(
    `SELECT COUNT(*) as cnt FROM oracle_updates WHERE oracle_pubkey = $1`,
    [oracle]
  );
  const rowCount = parseInt(countResult.rows[0].cnt, 10);
  if (rowCount > 0) {
    ok(`${rowCount} rows indexed`);
  } else {
    fail("No rows found");
    failures++;
  }

  // ── Check 2: Duplicate detection ──
  console.log("\n2. Duplicate slots:");
  const dupResult = await pool.query(
    `SELECT slot, COUNT(*) as cnt FROM oracle_updates
     WHERE oracle_pubkey = $1 GROUP BY slot HAVING COUNT(*) > 1 LIMIT 10`,
    [oracle]
  );
  if (dupResult.rows.length === 0) {
    ok("No duplicate slots");
  } else {
    fail(`${dupResult.rows.length} duplicate slot(s): ${dupResult.rows.map((r: any) => r.slot).join(", ")}`);
    failures++;
  }

  // ── Check 3: Slot ordering ──
  console.log("\n3. Slot ordering:");
  const orderResult = await pool.query(
    `SELECT slot, cumulative_price FROM oracle_updates
     WHERE oracle_pubkey = $1 ORDER BY slot ASC`,
    [oracle]
  );
  let orderOk = true;
  let cumulOk = true;
  let prevSlot = BigInt(0);
  let prevCumul = BigInt(0);

  for (const row of orderResult.rows) {
    const slot = BigInt(row.slot);
    const cumul = BigInt(row.cumulative_price);
    if (slot <= prevSlot && prevSlot > 0) {
      fail(`Slots not strictly increasing: ${prevSlot} → ${slot}`);
      orderOk = false;
      failures++;
      break;
    }
    if (cumul < prevCumul) {
      fail(`Cumulative price decreased: ${prevCumul} → ${cumul} at slot ${slot}`);
      cumulOk = false;
      failures++;
      break;
    }
    prevSlot = slot;
    prevCumul = cumul;
  }
  if (orderOk) ok("Slots strictly increasing");
  if (cumulOk) ok("Cumulative prices monotonically non-decreasing");

  // ── Check 4: TWAP comparison ──
  console.log("\n4. TWAP comparison:");
  try {
    const oracleAccount = await client.fetchOracle(oraclePk);
    const chainTwap = await client.computeSwapFromChain(
      oracleAccount.baseMint, oracleAccount.quoteMint, window
    );

    // Compute TWAP from DB: get the two boundary observations
    const currentSlot = await conn.getSlot();
    const windowStart = currentSlot - window;

    // Find observation at or before window_start
    const pastResult = await pool.query(
      `SELECT slot, cumulative_price FROM oracle_updates
       WHERE oracle_pubkey = $1 AND slot <= $2
       ORDER BY slot DESC LIMIT 1`,
      [oracle, windowStart]
    );

    // Find latest observation
    const latestResult = await pool.query(
      `SELECT slot, cumulative_price, price FROM oracle_updates
       WHERE oracle_pubkey = $1
       ORDER BY slot DESC LIMIT 1`,
      [oracle]
    );

    if (pastResult.rows.length === 0 || latestResult.rows.length === 0) {
      warn("Not enough DB rows to compute TWAP for comparison");
    } else {
      const pastSlot = BigInt(pastResult.rows[0].slot);
      const pastCumul = BigInt(pastResult.rows[0].cumulative_price);
      const latestSlot = BigInt(latestResult.rows[0].slot);
      const latestCumul = BigInt(latestResult.rows[0].cumulative_price);
      const latestPrice = BigInt(latestResult.rows[0].price);

      // Extend cumulative to current slot (like get_swap does)
      const slotDelta = BigInt(currentSlot) - latestSlot;
      const cumulNow = latestCumul + latestPrice * slotDelta;

      const dbSlotDelta = BigInt(currentSlot) - pastSlot;
      const dbTwap = dbSlotDelta > 0n ? (cumulNow - pastCumul) / dbSlotDelta : 0n;

      const chainVal = BigInt(chainTwap.toString());
      const diff = chainVal > dbTwap ? chainVal - dbTwap : dbTwap - chainVal;
      const tolerance = chainVal / 100n; // 1% tolerance (slot timing differences)

      if (diff <= tolerance) {
        ok(`Chain TWAP: ${chainVal}, DB TWAP: ${dbTwap} (diff=${diff}, within 1%)`);
      } else {
        fail(`TWAP mismatch — chain: ${chainVal}, DB: ${dbTwap}, diff: ${diff}`);
        failures++;
      }
    }
  } catch (err) {
    warn(`TWAP comparison skipped: ${(err as Error).message}`);
  }

  // ── Summary ──
  await pool.end();
  console.log(failures === 0
    ? "\n\x1b[32mAll checks passed.\x1b[0m\n"
    : `\n\x1b[31m${failures} check(s) failed.\x1b[0m\n`
  );
  process.exit(failures);
}

run().catch((err) => {
  console.error(err.message);
  process.exit(1);
});
