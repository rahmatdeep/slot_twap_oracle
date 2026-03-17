import { AnchorProvider, BN, Wallet } from "@coral-xyz/anchor";
import { Connection, Keypair, PublicKey } from "@solana/web3.js";
import {
  SlotTwapOracleClient,
} from "@slot-twap-oracle/sdk";
import {
  RPC_URL,
  ORACLE_PROGRAM_ID,
  BASE_MINT,
  QUOTE_MINT,
  RAYDIUM_AMM_ID,
  ORCA_WHIRLPOOL_ID,
  METEORA_POOL_ID,
  MIN_SOURCES,
  UPDATE_INTERVAL_MS,
  loadKeypair,
} from "./config";
import { fetchPrice as fetchRaydium } from "./sources/raydium";
import { fetchPrice as fetchOrca } from "./sources/orca";
import { fetchPrice as fetchMeteora } from "./sources/meteora";

const PRICE_DECIMALS = 9;

const MAX_RETRIES = 5;
const BASE_DELAY_MS = 1000;

const connection = new Connection(RPC_URL, "confirmed");
const payer = Keypair.fromSecretKey(loadKeypair());
const provider = new AnchorProvider(connection, new Wallet(payer), {
  commitment: "confirmed",
});

const client = new SlotTwapOracleClient(provider, ORACLE_PROGRAM_ID);
const [oraclePda] = client.findOraclePda(BASE_MINT, QUOTE_MINT);
const [observationBuffer] = client.findObservationBufferPda(oraclePda);

interface PriceSource {
  name: string;
  poolAddress: PublicKey;
  fetch: (connection: Connection, pool: PublicKey) => Promise<number>;
}

const sources: PriceSource[] = [];

if (RAYDIUM_AMM_ID) {
  sources.push({ name: "Raydium", poolAddress: RAYDIUM_AMM_ID, fetch: fetchRaydium });
}
if (ORCA_WHIRLPOOL_ID) {
  sources.push({ name: "Orca", poolAddress: ORCA_WHIRLPOOL_ID, fetch: fetchOrca });
}
if (METEORA_POOL_ID) {
  sources.push({ name: "Meteora", poolAddress: METEORA_POOL_ID, fetch: fetchMeteora });
}

if (sources.length < MIN_SOURCES) {
  throw new Error(
    `At least ${MIN_SOURCES} price sources must be configured, but only ${sources.length} found. ` +
      `Set RAYDIUM_AMM_ID, ORCA_WHIRLPOOL_ID, and/or METEORA_POOL_ID in .env`
  );
}

console.log(`[updater] Oracle PDA: ${oraclePda.toBase58()}`);
console.log(`[updater] Observation buffer: ${observationBuffer.toBase58()}`);
console.log(`[updater] Price sources: ${sources.map((s) => s.name).join(", ")}`);
console.log(`[updater] Min required sources: ${MIN_SOURCES}`);
console.log(`[updater] Update interval: ${UPDATE_INTERVAL_MS / 1000}s`);

function median(values: number[]): number {
  const sorted = [...values].sort((a, b) => a - b);
  const mid = Math.floor(sorted.length / 2);
  if (sorted.length % 2 === 0) {
    return (sorted[mid - 1] + sorted[mid]) / 2;
  }
  return sorted[mid];
}

async function retryWithBackoff<T>(fn: () => Promise<T>): Promise<T> {
  for (let attempt = 0; attempt < MAX_RETRIES; attempt++) {
    try {
      return await fn();
    } catch (err) {
      const isLastAttempt = attempt === MAX_RETRIES - 1;
      if (isLastAttempt) throw err;

      const delayMs = BASE_DELAY_MS * 2 ** attempt;
      console.warn(
        `[updater] Attempt ${attempt + 1}/${MAX_RETRIES} failed: ${(err as Error).message}. ` +
          `Retrying in ${delayMs / 1000}s...`
      );
      await new Promise((resolve) => setTimeout(resolve, delayMs));
    }
  }
  throw new Error("unreachable");
}

async function fetchAllPrices(): Promise<number[]> {
  const results = await Promise.allSettled(
    sources.map(async (source) => {
      const price = await source.fetch(connection, source.poolAddress);
      console.log(`[updater]   ${source.name}: ${price}`);
      return price;
    })
  );

  const prices: number[] = [];
  for (let i = 0; i < results.length; i++) {
    const result = results[i];
    if (result.status === "fulfilled") {
      prices.push(result.value);
    } else {
      console.warn(
        `[updater]   ${sources[i].name}: FAILED - ${result.reason?.message ?? result.reason}`
      );
    }
  }

  return prices;
}

function toScaledBigint(price: number): bigint {
  // Scale float to integer with PRICE_DECIMALS precision
  return BigInt(Math.round(price * 10 ** PRICE_DECIMALS));
}

async function tick(): Promise<void> {
  try {
    console.log("[updater] Fetching prices...");
    const prices = await fetchAllPrices();

    if (prices.length < MIN_SOURCES) {
      console.warn(
        `[updater] Only ${prices.length}/${sources.length} sources returned a price ` +
          `(need >= ${MIN_SOURCES}). Skipping update.`
      );
      return;
    }

    const medianPrice = median(prices);
    const scaledPrice = toScaledBigint(medianPrice);

    console.log(
      `[updater] Median price: ${medianPrice} (${prices.length} sources) -> scaled: ${scaledPrice}`
    );

    const sig = await retryWithBackoff(() =>
      client.updatePrice(oraclePda, new BN(scaledPrice.toString()), payer)
    );
    console.log(`[updater] update_price tx: ${sig}`);
  } catch (err) {
    console.error(
      `[updater] Failed after ${MAX_RETRIES} retries: ${(err as Error).message}`
    );
  }
}

async function main(): Promise<void> {
  console.log("[updater] Starting updater bot...");

  await tick();
  setInterval(tick, UPDATE_INTERVAL_MS);
}

main();
