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

async function tick(): Promise<void> {
  try {
    const price = await fetchRaydiumPrice(connection, RAYDIUM_AMM_ID);
    console.log(
      `[updater] Fetched price: ${price} (${Number(price) / 10 ** PRICE_DECIMALS} scaled)`
    );

    const sig = await client.updatePrice(oraclePda, new BN(price.toString()), payer);
    console.log(`[updater] update_price tx: ${sig}`);
  } catch (err) {
    console.error(`[updater] Error: ${(err as Error).message}`);
  }
}

async function main(): Promise<void> {
  console.log("[updater] Starting updater bot...");

  await tick();
  setInterval(tick, UPDATE_INTERVAL_MS);
}

main();
