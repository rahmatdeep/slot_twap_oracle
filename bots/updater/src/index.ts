import { AnchorProvider, BN, Wallet } from "@coral-xyz/anchor";
import { Connection, Keypair, PublicKey } from "@solana/web3.js";
import { SlotTwapOracleClient } from "@slot-twap-oracle/sdk";
import {
  RPC_URL,
  ORACLE_PROGRAM_ID,
  MIN_SOURCES,
  UPDATE_INTERVAL_MS,
  loadKeypair,
  loadPairs,
  PairConfig,
} from "./config";
import { fetchPrice as fetchRaydium } from "./sources/raydium";
import { fetchPrice as fetchOrca } from "./sources/orca";
import { fetchPrice as fetchMeteora } from "./sources/meteora";
import { PersistentMetrics } from "./metrics";

const PRICE_DECIMALS = 9;
const MAX_RETRIES = 5;
const BASE_DELAY_MS = 1000;
const MAX_SOURCE_SPREAD = 0.05; // 5%
const STALE_ORACLE_SLOTS = 100;
const METRICS_INTERVAL_MS = 5 * 60 * 1000; // 5 minutes

// ── Logging ──

function ts(): string {
  return new Date().toISOString();
}

function log(pair: string, msg: string): void {
  console.log(`${ts()} [${pair}] ${msg}`);
}

function warn(pair: string, msg: string): void {
  console.warn(`${ts()} [${pair}] ${msg}`);
}

function error(pair: string, msg: string): void {
  console.error(`${ts()} [${pair}] ${msg}`);
}

// ── Metrics ──

const metrics = new PersistentMetrics();

// ── Setup ──

const connection = new Connection(RPC_URL, "confirmed");
const payer = Keypair.fromSecretKey(loadKeypair());
const provider = new AnchorProvider(connection, new Wallet(payer), {
  commitment: "confirmed",
});
const client = new SlotTwapOracleClient(provider, ORACLE_PROGRAM_ID);

type FetchFn = (conn: Connection, pool: PublicKey, baseMint: PublicKey, quoteMint: PublicKey) => Promise<number>;

interface PriceSource {
  name: string;
  poolAddress: PublicKey;
  fetch: FetchFn;
}

const FETCHERS: Record<string, FetchFn> = {
  raydium: fetchRaydium,
  orca: fetchOrca,
  meteora: fetchMeteora,
};

interface Pair {
  name: string;
  oracle: PublicKey;
  baseMint: PublicKey;
  quoteMint: PublicKey;
  sources: PriceSource[];
}

function buildPairs(configs: PairConfig[]): Pair[] {
  return configs.map((cfg) => {
    const sources: PriceSource[] = [];
    for (const [key, address] of Object.entries(cfg.sources)) {
      const fetcher = FETCHERS[key];
      if (!fetcher) {
        throw new Error(`[${cfg.name}] Unknown source "${key}". Valid: ${Object.keys(FETCHERS).join(", ")}`);
      }
      if (address) {
        sources.push({
          name: key,
          poolAddress: new PublicKey(address),
          fetch: fetcher,
        });
      }
    }
    if (sources.length < MIN_SOURCES) {
      throw new Error(
        `[${cfg.name}] Needs >= ${MIN_SOURCES} sources, but only ${sources.length} configured`
      );
    }
    return {
      name: cfg.name,
      oracle: new PublicKey(cfg.oracle),
      baseMint: new PublicKey(cfg.baseMint),
      quoteMint: new PublicKey(cfg.quoteMint),
      sources,
    };
  });
}

const pairs = buildPairs(loadPairs());

log("updater", `Loaded ${pairs.length} pair(s)`);
for (const pair of pairs) {
  log("updater", `  ${pair.name}: oracle=${pair.oracle.toBase58()}, sources=${pair.sources.map((s) => s.name).join(", ")}`);
}
log("updater", `Min required sources: ${MIN_SOURCES}`);
log("updater", `Update interval: ${UPDATE_INTERVAL_MS / 1000}s`);

// ── Core logic ──

function median(values: number[]): number {
  const sorted = [...values].sort((a, b) => a - b);
  const mid = Math.floor(sorted.length / 2);
  if (sorted.length % 2 === 0) {
    return (sorted[mid - 1] + sorted[mid]) / 2;
  }
  return sorted[mid];
}

async function retryWithBackoff<T>(fn: () => Promise<T>, label: string): Promise<T> {
  for (let attempt = 0; attempt < MAX_RETRIES; attempt++) {
    try {
      return await fn();
    } catch (err) {
      const isLastAttempt = attempt === MAX_RETRIES - 1;
      if (isLastAttempt) throw err;

      const delayMs = BASE_DELAY_MS * 2 ** attempt;
      warn(
        label,
        `Attempt ${attempt + 1}/${MAX_RETRIES} failed: ${(err as Error).message}. Retrying in ${delayMs / 1000}s...`
      );
      await new Promise((resolve) => setTimeout(resolve, delayMs));
    }
  }
  throw new Error("unreachable");
}

async function fetchPricesForPair(pair: Pair): Promise<number[]> {
  const results = await Promise.allSettled(
    pair.sources.map(async (source) => {
      const price = await source.fetch(connection, source.poolAddress, pair.baseMint, pair.quoteMint);
      log(pair.name, `  ${source.name}: ${price}`);
      return price;
    })
  );

  const prices: number[] = [];
  for (let i = 0; i < results.length; i++) {
    const result = results[i];
    if (result.status === "fulfilled") {
      prices.push(result.value);
    } else {
      warn(
        pair.name,
        `  ${pair.sources[i].name}: FAILED - ${result.reason?.message ?? result.reason}`
      );
    }
  }
  return prices;
}

function toScaledBigint(price: number): bigint {
  return BigInt(Math.round(price * 10 ** PRICE_DECIMALS));
}

async function checkStaleness(pair: Pair, currentSlot: number): Promise<void> {
  const lastSlot = metrics.getLastUpdateSlot(pair.name);
  if (lastSlot !== undefined && currentSlot - lastSlot > STALE_ORACLE_SLOTS) {
    warn(
      pair.name,
      `Oracle stale: last update at slot ${lastSlot}, current slot ${currentSlot} (${currentSlot - lastSlot} slots behind)`
    );
  }
}

async function updatePair(pair: Pair): Promise<void> {
  log(pair.name, "Fetching prices...");
  const prices = await fetchPricesForPair(pair);

  if (prices.length === 0) {
    error(pair.name, "All sources failed. Skipping.");
    metrics.recordSkip();
    return;
  }

  if (prices.length < MIN_SOURCES) {
    warn(
      pair.name,
      `Only ${prices.length}/${pair.sources.length} sources (need >= ${MIN_SOURCES}). Skipping.`
    );
    metrics.recordSkip();
    return;
  }

  const medianPrice = median(prices);
  const minPrice = Math.min(...prices);
  const maxPrice = Math.max(...prices);
  const spread = (maxPrice - minPrice) / medianPrice;

  if (spread > MAX_SOURCE_SPREAD) {
    warn(
      pair.name,
      `High source deviation: spread=${(spread * 100).toFixed(2)}% (min=${minPrice}, max=${maxPrice}, median=${medianPrice}). Skipping.`
    );
    metrics.recordSkip();
    return;
  }

  const scaledPrice = toScaledBigint(medianPrice);

  log(
    pair.name,
    `Median: ${medianPrice} (${prices.length} sources, spread=${(spread * 100).toFixed(2)}%) -> scaled: ${scaledPrice}`
  );

  const sig = await retryWithBackoff(
    () => client.updatePrice(pair.oracle, new BN(scaledPrice.toString()), payer),
    pair.name
  );

  const currentSlot = await connection.getSlot();
  metrics.recordSuccess(pair.name, currentSlot);

  log(pair.name, `update_price tx: ${sig} (slot=${currentSlot}, price=${medianPrice})`);
}

async function tick(): Promise<void> {
  const currentSlot = await connection.getSlot().catch(() => 0);

  // Check staleness for all pairs before updating
  if (currentSlot > 0) {
    for (const pair of pairs) {
      await checkStaleness(pair, currentSlot);
    }
  }

  const results = await Promise.allSettled(pairs.map((pair) => updatePair(pair)));

  for (let i = 0; i < results.length; i++) {
    const result = results[i];
    if (result.status === "rejected") {
      error(
        pairs[i].name,
        `Failed after ${MAX_RETRIES} retries: ${result.reason?.message ?? result.reason}`
      );
      metrics.recordFailure();
    }
  }
}

let tickTimer: ReturnType<typeof setInterval> | null = null;
let metricsTimer: ReturnType<typeof setInterval> | null = null;
let shuttingDown = false;

function shutdown(signal: string): void {
  if (shuttingDown) return;
  shuttingDown = true;

  log("updater", `Received ${signal}, shutting down...`);

  if (tickTimer) clearInterval(tickTimer);
  if (metricsTimer) clearInterval(metricsTimer);

  metrics.flush();
  metrics.log(ts());

  log("updater", "Shutdown complete.");
  process.exit(0);
}

process.on("SIGINT", () => shutdown("SIGINT"));
process.on("SIGTERM", () => shutdown("SIGTERM"));

async function main(): Promise<void> {
  log("updater", "Starting updater bot...");

  metricsTimer = setInterval(() => metrics.log(ts()), METRICS_INTERVAL_MS);

  await tick();
  tickTimer = setInterval(tick, UPDATE_INTERVAL_MS);
}

main();
