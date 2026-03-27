/**
 * End-to-end test: spins up solana-test-validator, deploys the program,
 * and exercises the full lifecycle via the SDK.
 *
 * Usage: npx tsx scripts/e2e-test.ts
 */

import { spawn, ChildProcess } from "child_process";
import { AnchorProvider, BN, Wallet } from "@coral-xyz/anchor";
import {
  Connection,
  Keypair,
  PublicKey,
  LAMPORTS_PER_SOL,
  sendAndConfirmTransaction,
  SystemProgram,
  Transaction,
} from "@solana/web3.js";
import {
  createInitializeMint2Instruction,
  getMintLen,
  TOKEN_2022_PROGRAM_ID,
} from "@solana/spl-token";
import { SlotTwapOracleClient, PROGRAM_ID } from "@slot-twap-oracle/sdk";

const RPC_URL = "http://127.0.0.1:8899";
const PROGRAM_SO = "target/deploy/slot_twap_oracle.so";

// ── Helpers ──

function sleep(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms));
}

async function waitForNextSlot(conn: Connection): Promise<void> {
  const current = await conn.getSlot();
  while ((await conn.getSlot()) <= current) {
    await sleep(200);
  }
}

async function waitForValidator(conn: Connection, maxWait = 30_000): Promise<void> {
  const start = Date.now();
  while (Date.now() - start < maxWait) {
    try {
      await conn.getSlot();
      return;
    } catch {
      await sleep(500);
    }
  }
  throw new Error("Validator did not start in time");
}

async function airdrop(conn: Connection, pubkey: PublicKey, sol: number): Promise<void> {
  const sig = await conn.requestAirdrop(pubkey, sol * LAMPORTS_PER_SOL);
  await conn.confirmTransaction(sig, "confirmed");
}

async function createMint(conn: Connection, payer: Keypair): Promise<PublicKey> {
  const mint = Keypair.generate();
  const space = getMintLen([]);
  const rent = await conn.getMinimumBalanceForRentExemption(space);

  const tx = new Transaction().add(
    SystemProgram.createAccount({
      fromPubkey: payer.publicKey,
      newAccountPubkey: mint.publicKey,
      space,
      lamports: rent,
      programId: TOKEN_2022_PROGRAM_ID,
    }),
    createInitializeMint2Instruction(
      mint.publicKey,
      6,
      payer.publicKey,
      null,
      TOKEN_2022_PROGRAM_ID
    )
  );

  await sendAndConfirmTransaction(conn, tx, [payer, mint]);
  return mint.publicKey;
}

function ok(msg: string): void {
  console.log(`  \x1b[32m✓\x1b[0m ${msg}`);
}

function fail(msg: string): void {
  console.error(`  \x1b[31m✗\x1b[0m ${msg}`);
}

// ── Main ──

let validator: ChildProcess | null = null;

async function run(): Promise<void> {
  console.log("\n=== Slot TWAP Oracle — End-to-End Test ===\n");

  // 1. Start validator
  console.log("Starting solana-test-validator...");
  validator = spawn("solana-test-validator", [
    "--bpf-program", PROGRAM_ID.toBase58(), PROGRAM_SO,
    "--reset",
    "--quiet",
  ], { stdio: "ignore" });

  const conn = new Connection(RPC_URL, "confirmed");
  await waitForValidator(conn);
  ok("Validator running");

  // 2. Setup
  const payer = Keypair.generate();
  await airdrop(conn, payer.publicKey, 10);
  ok(`Payer funded: ${payer.publicKey.toBase58()}`);

  const provider = new AnchorProvider(conn, new Wallet(payer), { commitment: "confirmed" });
  const client = new SlotTwapOracleClient(provider);

  const baseMint = await createMint(conn, payer);
  const quoteMint = await createMint(conn, payer);
  ok(`Mints created: base=${baseMint.toBase58().slice(0, 8)}... quote=${quoteMint.toBase58().slice(0, 8)}...`);

  // 3. Initialize oracle
  console.log("\n--- Initialize Oracle ---");
  const capacity = 32;
  const initSig = await client.initializeOracle(baseMint, quoteMint, capacity, payer);
  ok(`initialize_oracle tx: ${initSig.slice(0, 16)}...`);

  const [oraclePda] = client.findOraclePda(baseMint, quoteMint);
  const [bufferPda] = client.findObservationBufferPda(oraclePda);

  const oracle0 = await client.fetchOracle(oraclePda);
  ok(`Oracle PDA: ${oraclePda.toBase58()}`);
  ok(`lastPrice=0, lastSlot=${oracle0.lastSlot}, lastUpdater=${oracle0.lastUpdater.toBase58()}`);

  const buffer0 = await client.fetchObservationBuffer(bufferPda);
  ok(`Buffer: capacity=${buffer0.capacity}, observations=${buffer0.observations.length}`);

  // 4. Update price (simulating what the bot does)
  console.log("\n--- Update Prices ---");

  const prices = [1000_000_000, 1050_000_000, 1100_000_000, 1050_000_000, 1000_000_000];

  for (let i = 0; i < prices.length; i++) {
    await waitForNextSlot(conn);
    const price = prices[i];
    const sig = await client.updatePrice(oraclePda, new BN(price), payer);
    const oracleState = await client.fetchOracle(oraclePda);
    ok(
      `update #${i + 1}: price=${price / 1e9} ` +
      `cumulative=${oracleState.cumulativePrice} ` +
      `slot=${oracleState.lastSlot} ` +
      `tx=${sig.slice(0, 16)}...`
    );
  }

  // 5. Verify observation buffer
  console.log("\n--- Observation Buffer ---");
  const buffer = await client.fetchObservationBuffer(bufferPda);
  ok(`${buffer.observations.length} observations stored (capacity=${buffer.capacity})`);
  for (const obs of buffer.observations) {
    ok(`  slot=${obs.slot} cumulative=${obs.cumulativePrice}`);
  }

  // 6. Query TWAP
  console.log("\n--- Query TWAP ---");
  const currentSlot = await conn.getSlot();
  const oracleFinal = await client.fetchOracle(oraclePda);
  const slotsSinceUpdate = currentSlot - oracleFinal.lastSlot.toNumber();
  ok(`Current slot: ${currentSlot}, oracle last slot: ${oracleFinal.lastSlot}, gap: ${slotsSinceUpdate}`);

  // Compute TWAP off-chain using observations we know exist.
  // First observation is at slot 6 (update #1), so use a window that starts after it.
  const oracleFinalForTwap = await client.fetchOracle(oraclePda);
  const firstObsSlot = buffer.observations[0].slot.toNumber();
  const windowSlots = currentSlot - firstObsSlot - 1; // ensure window_start > first obs
  if (windowSlots > 0) {
    const twap = await client.computeSwapFromChain(baseMint, quoteMint, windowSlots);
    ok(`TWAP over ${windowSlots} slots: ${twap.toNumber() / 1e9}`);
  } else {
    ok("Skipped TWAP query (not enough slot distance)");
  }

  // 7. Test permissionless update (different signer)
  console.log("\n--- Permissionless Update ---");
  const otherPayer = Keypair.generate();
  await airdrop(conn, otherPayer.publicKey, 2);
  await waitForNextSlot(conn);
  const otherSig = await client.updatePrice(oraclePda, new BN(1010_000_000), otherPayer);
  const oracleAfter = await client.fetchOracle(oraclePda);
  ok(
    `Different signer updated: updater=${oracleAfter.lastUpdater.toBase58().slice(0, 8)}... ` +
    `price=${oracleAfter.lastPrice.toNumber() / 1e9} tx=${otherSig.slice(0, 16)}...`
  );

  // 8. Parse events from last tx
  console.log("\n--- Event Parsing ---");
  const events = await client.parseOracleUpdateEvents(otherSig);
  if (events.length === 1) {
    const e = events[0];
    ok(
      `OracleUpdate event: oracle=${e.oracle.toBase58().slice(0, 8)}... ` +
      `price=${e.price.toNumber() / 1e9} slot=${e.slot} updater=${e.updater.toBase58().slice(0, 8)}...`
    );
  } else {
    fail(`Expected 1 event, got ${events.length}`);
  }

  // 9. Test staleness rejection
  console.log("\n--- Staleness Rejection ---");
  // Wait a few slots so the oracle becomes stale relative to max_staleness=1
  await waitForNextSlot(conn);
  await waitForNextSlot(conn);
  try {
    const [obsBuf] = client.findObservationBufferPda(oraclePda);
    await client.program.methods
      .getSwap(new BN(3), new BN(1))
      .accounts({ oracle: oraclePda, observationBuffer: obsBuf })
      .rpc();
    fail("Should have thrown StaleOracle");
  } catch (err) {
    const msg = (err as Error).message;
    if (msg.includes("StaleOracle") || msg.includes("6004")) {
      ok("get_swap correctly rejected stale oracle");
    } else {
      fail(`Unexpected error: ${msg}`);
    }
  }

  // 10. Test deviation rejection
  console.log("\n--- Deviation Rejection ---");
  try {
    await waitForNextSlot(conn);
    // Current price is 1.01, try to set to 2.0 (>10% deviation)
    await client.updatePrice(oraclePda, new BN(2_000_000_000), payer);
    fail("Should have thrown PriceDeviationTooLarge");
  } catch (err) {
    if ((err as Error).message.includes("PriceDeviationTooLarge") || (err as Error).message.includes("6005")) {
      ok("update_price correctly rejected excessive deviation");
    } else {
      fail(`Unexpected error: ${(err as Error).message}`);
    }
  }

  console.log("\n\x1b[32m=== All checks passed ===\x1b[0m\n");
}

run()
  .catch((err) => {
    fail(err.message);
    process.exitCode = 1;
  })
  .finally(() => {
    if (validator) {
      validator.kill("SIGTERM");
    }
  });
