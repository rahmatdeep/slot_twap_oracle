import { PublicKey } from "@solana/web3.js";
import dotenv from "dotenv";
import path from "path";
import fs from "fs";

dotenv.config({ path: path.resolve(__dirname, "../.env") });

function requireEnv(key: string): string {
  const val = process.env[key];
  if (!val) throw new Error(`Missing env var: ${key}`);
  return val;
}

export const RPC_URL = requireEnv("RPC_URL");
export const KEYPAIR_PATH = requireEnv("KEYPAIR_PATH").replace(
  "~",
  process.env.HOME || ""
);
export const ORACLE_PROGRAM_ID = new PublicKey(requireEnv("ORACLE_PROGRAM_ID"));
export const BASE_MINT = new PublicKey(requireEnv("BASE_MINT"));
export const QUOTE_MINT = new PublicKey(requireEnv("QUOTE_MINT"));
function optionalPubkey(key: string): PublicKey | null {
  const val = process.env[key];
  if (!val) return null;
  return new PublicKey(val);
}

export const RAYDIUM_AMM_ID = optionalPubkey("RAYDIUM_AMM_ID");
export const ORCA_WHIRLPOOL_ID = optionalPubkey("ORCA_WHIRLPOOL_ID");
export const METEORA_POOL_ID = optionalPubkey("METEORA_POOL_ID");

export const MIN_SOURCES = parseInt(process.env.MIN_SOURCES || "2", 10);

export const UPDATE_INTERVAL_MS = parseInt(
  process.env.UPDATE_INTERVAL_MS || "30000",
  10
);

export function loadKeypairFromFile(filePath: string): Uint8Array {
  if (!fs.existsSync(filePath)) {
    throw new Error(`Keypair file not found: ${filePath}`);
  }

  let raw: string;
  try {
    raw = fs.readFileSync(filePath, "utf-8");
  } catch (err) {
    throw new Error(`Failed to read keypair file ${filePath}: ${(err as Error).message}`);
  }

  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    throw new Error(`Keypair file ${filePath} is not valid JSON`);
  }

  if (
    !Array.isArray(parsed) ||
    parsed.length !== 64 ||
    !parsed.every((b) => Number.isInteger(b) && b >= 0 && b <= 255)
  ) {
    throw new Error(
      `Keypair file ${filePath} does not contain a valid secret key array ` +
        `(expected a JSON array of 64 integers in 0..255)`
    );
  }

  return Uint8Array.from(parsed);
}

export function loadKeypair(): Uint8Array {
  return loadKeypairFromFile(KEYPAIR_PATH);
}
