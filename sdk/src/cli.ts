#!/usr/bin/env node

import { Command } from "commander";
import { AnchorProvider, BN, Wallet } from "@coral-xyz/anchor";
import { Connection, Keypair, PublicKey } from "@solana/web3.js";
import fs from "fs";

import { SlotTwapOracleClient } from "./client";
import { findOraclePda, PROGRAM_ID } from "./pda";

// ── Helpers ──

function loadKeypair(path: string): Keypair {
  const resolved = path.replace("~", process.env.HOME || "");
  const raw = fs.readFileSync(resolved, "utf-8");
  const bytes = Uint8Array.from(JSON.parse(raw));
  return Keypair.fromSecretKey(bytes);
}

function makeClient(rpc: string, keypairPath: string, programId?: string): {
  client: SlotTwapOracleClient;
  payer: Keypair;
} {
  const payer = loadKeypair(keypairPath);
  const connection = new Connection(rpc, "confirmed");
  const provider = new AnchorProvider(connection, new Wallet(payer), {
    commitment: "confirmed",
  });
  const pid = programId ? new PublicKey(programId) : PROGRAM_ID;
  const client = new SlotTwapOracleClient(provider, pid);
  return { client, payer };
}

function parsePubkey(value: string, name: string): PublicKey {
  try {
    return new PublicKey(value);
  } catch {
    throw new Error(`Invalid ${name} pubkey: ${value}`);
  }
}

// ── CLI ──

const program = new Command()
  .name("slot-twap-oracle")
  .description("CLI for the Slot TWAP Oracle program")
  .version("0.1.0")
  .option("-r, --rpc <url>", "Solana RPC URL", "http://127.0.0.1:8899")
  .option("-k, --keypair <path>", "Path to payer keypair JSON", "~/.config/solana/id.json")
  .option("-p, --program-id <pubkey>", "Oracle program ID override");

// ── init ──

program
  .command("init")
  .description("Initialize a new oracle for a trading pair")
  .requiredOption("--base-mint <pubkey>", "Base token mint address")
  .requiredOption("--quote-mint <pubkey>", "Quote token mint address")
  .option("--capacity <number>", "Observation buffer capacity", "32")
  .action(async (opts) => {
    const { rpc, keypair, programId } = program.opts();
    const { client, payer } = makeClient(rpc, keypair, programId);

    const baseMint = parsePubkey(opts.baseMint, "base-mint");
    const quoteMint = parsePubkey(opts.quoteMint, "quote-mint");
    const capacity = parseInt(opts.capacity, 10);
    if (capacity <= 0 || isNaN(capacity)) {
      throw new Error("Capacity must be a positive integer");
    }

    const [oraclePda] = client.findOraclePda(baseMint, quoteMint);
    console.log(`Initializing oracle: ${oraclePda.toBase58()}`);
    console.log(`  base_mint:  ${baseMint.toBase58()}`);
    console.log(`  quote_mint: ${quoteMint.toBase58()}`);
    console.log(`  capacity:   ${capacity}`);

    const sig = await client.initializeOracle(baseMint, quoteMint, capacity, payer);
    console.log(`\nTransaction: ${sig}`);
    console.log(`Oracle PDA:  ${oraclePda.toBase58()}`);
  });

// ── update-price ──

program
  .command("update-price")
  .description("Submit a price update to an oracle")
  .requiredOption("--oracle <pubkey>", "Oracle PDA address")
  .requiredOption("--price <number>", "New price (integer, scaled)")
  .action(async (opts) => {
    const { rpc, keypair, programId } = program.opts();
    const { client, payer } = makeClient(rpc, keypair, programId);

    const oracle = parsePubkey(opts.oracle, "oracle");
    const price = new BN(opts.price);
    if (price.isNeg()) {
      throw new Error("Price must be non-negative");
    }

    console.log(`Updating oracle: ${oracle.toBase58()}`);
    console.log(`  price: ${price.toString()}`);

    const sig = await client.updatePrice(oracle, price, payer);
    console.log(`\nTransaction: ${sig}`);
  });

// ── get-swap ──

program
  .command("get-swap")
  .description("Query TWAP for an oracle over a slot window")
  .requiredOption("--base-mint <pubkey>", "Base token mint address")
  .requiredOption("--quote-mint <pubkey>", "Quote token mint address")
  .requiredOption("--window <slots>", "Window size in slots")
  .action(async (opts) => {
    const { rpc, keypair, programId } = program.opts();
    const { client } = makeClient(rpc, keypair, programId);

    const baseMint = parsePubkey(opts.baseMint, "base-mint");
    const quoteMint = parsePubkey(opts.quoteMint, "quote-mint");
    const window = parseInt(opts.window, 10);
    if (window <= 0 || isNaN(window)) {
      throw new Error("Window must be a positive integer");
    }

    const [oraclePda] = client.findOraclePda(baseMint, quoteMint);
    console.log(`Querying TWAP for oracle: ${oraclePda.toBase58()}`);
    console.log(`  window: ${window} slots`);

    const twap = await client.computeSwapFromChain(baseMint, quoteMint, window);
    console.log(`\nTWAP: ${twap.toString()}`);
  });

// ── parse-events ──

program
  .command("parse-events")
  .description("Parse OracleUpdate events from a transaction or oracle history")
  .option("--tx <signature>", "Transaction signature to parse")
  .option("--oracle <pubkey>", "Oracle address to fetch recent events for")
  .option("--limit <number>", "Number of recent transactions to scan", "20")
  .action(async (opts) => {
    const { rpc, keypair, programId } = program.opts();
    const { client } = makeClient(rpc, keypair, programId);

    if (!opts.tx && !opts.oracle) {
      throw new Error("Provide --tx or --oracle");
    }

    let events;
    if (opts.tx) {
      console.log(`Parsing events from tx: ${opts.tx}`);
      events = await client.parseOracleUpdateEvents(opts.tx);
    } else {
      const oracle = parsePubkey(opts.oracle!, "oracle");
      const limit = parseInt(opts.limit, 10);
      console.log(`Fetching last ${limit} events for oracle: ${oracle.toBase58()}`);
      events = await client.getOracleUpdates(oracle, limit);
    }

    if (events.length === 0) {
      console.log("\nNo OracleUpdate events found.");
      return;
    }

    console.log(`\nFound ${events.length} event(s):\n`);
    for (const e of events) {
      console.log(`  oracle:          ${e.oracle.toBase58()}`);
      console.log(`  price:           ${e.price.toString()}`);
      console.log(`  cumulativePrice: ${e.cumulativePrice.toString()}`);
      console.log(`  slot:            ${e.slot.toString()}`);
      console.log(`  updater:         ${e.updater.toBase58()}`);
      console.log();
    }
  });

// ── inspect ──

program
  .command("inspect")
  .description("Fetch and display oracle state")
  .option("--oracle <pubkey>", "Oracle PDA address")
  .option("--base-mint <pubkey>", "Base token mint (derive PDA)")
  .option("--quote-mint <pubkey>", "Quote token mint (derive PDA)")
  .action(async (opts) => {
    const { rpc, keypair, programId } = program.opts();
    const { client } = makeClient(rpc, keypair, programId);

    let oraclePda: PublicKey;
    if (opts.oracle) {
      oraclePda = parsePubkey(opts.oracle, "oracle");
    } else if (opts.baseMint && opts.quoteMint) {
      const baseMint = parsePubkey(opts.baseMint, "base-mint");
      const quoteMint = parsePubkey(opts.quoteMint, "quote-mint");
      [oraclePda] = client.findOraclePda(baseMint, quoteMint);
    } else {
      throw new Error("Provide --oracle or both --base-mint and --quote-mint");
    }

    const oracle = await client.fetchOracle(oraclePda);
    const [bufferPda] = client.findObservationBufferPda(oraclePda);
    const buffer = await client.fetchObservationBuffer(bufferPda);

    console.log(`Oracle: ${oraclePda.toBase58()}`);
    console.log(`  owner:            ${oracle.owner.toBase58()}`);
    console.log(`  baseMint:         ${oracle.baseMint.toBase58()}`);
    console.log(`  quoteMint:        ${oracle.quoteMint.toBase58()}`);
    console.log(`  lastPrice:        ${oracle.lastPrice.toString()}`);
    console.log(`  cumulativePrice:  ${oracle.cumulativePrice.toString()}`);
    console.log(`  lastSlot:         ${oracle.lastSlot.toString()}`);
    console.log(`  lastUpdater:      ${oracle.lastUpdater.toBase58()}`);
    console.log(`  paused:           ${oracle.paused}`);
    console.log(`  maxDeviationBps:  ${oracle.maxDeviationBps}`);
    console.log(`\nBuffer: ${bufferPda.toBase58()}`);
    console.log(`  capacity:      ${buffer.capacity}`);
    console.log(`  observations:  ${buffer.observations.length}`);
    console.log(`  head:          ${buffer.head}`);
  });

program.parseAsync().catch((err) => {
  console.error(`Error: ${err.message}`);
  process.exit(1);
});
