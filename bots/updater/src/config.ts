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
export const RAYDIUM_AMM_ID = new PublicKey(requireEnv("RAYDIUM_AMM_ID"));
export const UPDATE_INTERVAL_MS = parseInt(
  process.env.UPDATE_INTERVAL_MS || "30000",
  10
);

export function loadKeypair(): Uint8Array {
  const raw = fs.readFileSync(KEYPAIR_PATH, "utf-8");
  return Uint8Array.from(JSON.parse(raw));
}
