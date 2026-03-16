import { AnchorProvider, BN, Wallet } from "@coral-xyz/anchor";
import { Connection, Keypair } from "@solana/web3.js";
import {
  SlotTwapOracleClient,
  findOraclePda,
} from "@slot-twap-oracle/sdk";
import {
  RPC_URL,
  ORACLE_PROGRAM_ID,
  BASE_MINT,
  QUOTE_MINT,
  RAYDIUM_AMM_ID,
  UPDATE_INTERVAL_MS,
  loadKeypair,
} from "./config";
import { fetchRaydiumPrice, PRICE_DECIMALS } from "./raydium";

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

console.log(`[updater] Oracle PDA: ${oraclePda.toBase58()}`);
console.log(`[updater] Observation buffer: ${observationBuffer.toBase58()}`);
console.log(`[updater] Raydium AMM: ${RAYDIUM_AMM_ID.toBase58()}`);
console.log(`[updater] Update interval: ${UPDATE_INTERVAL_MS / 1000}s`);

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

async function tick(): Promise<void> {
  try {
    const price = await fetchRaydiumPrice(connection, RAYDIUM_AMM_ID);
    console.log(
      `[updater] Fetched price: ${price} (${Number(price) / 10 ** PRICE_DECIMALS} scaled)`
    );

    const sig = await retryWithBackoff(() =>
      client.updatePrice(oraclePda, new BN(price.toString()), payer)
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
