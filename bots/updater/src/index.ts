import { Connection, Keypair } from "@solana/web3.js";
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
import {
  deriveOraclePda,
  deriveObservationBufferPda,
  buildUpdatePriceIx,
} from "./oracleClient";
import { sendTransaction } from "./sender";

const connection = new Connection(RPC_URL, "confirmed");
const payer = Keypair.fromSecretKey(loadKeypair());

const [oraclePda] = deriveOraclePda(BASE_MINT, QUOTE_MINT, ORACLE_PROGRAM_ID);
const [observationBuffer] = deriveObservationBufferPda(
  oraclePda,
  ORACLE_PROGRAM_ID
);

console.log(`[updater] Oracle PDA: ${oraclePda.toBase58()}`);
console.log(`[updater] Observation buffer: ${observationBuffer.toBase58()}`);
console.log(`[updater] Raydium AMM: ${RAYDIUM_AMM_ID.toBase58()}`);
console.log(
  `[updater] Update interval: ${UPDATE_INTERVAL_MS / 1000}s`
);

async function tick(): Promise<void> {
  try {
    const price = await fetchRaydiumPrice(connection, RAYDIUM_AMM_ID);
    console.log(
      `[updater] Fetched price: ${price} (${Number(price) / 10 ** PRICE_DECIMALS} scaled)`
    );

    const ix = buildUpdatePriceIx(
      oraclePda,
      observationBuffer,
      ORACLE_PROGRAM_ID,
      price
    );

    const sig = await sendTransaction(connection, payer, [ix]);
    console.log(`[updater] update_price tx: ${sig}`);
  } catch (err) {
    console.error(`[updater] Error: ${(err as Error).message}`);
  }
}

async function main(): Promise<void> {
  console.log("[updater] Starting updater bot...");

  // Run immediately, then on interval
  await tick();
  setInterval(tick, UPDATE_INTERVAL_MS);
}

main();
