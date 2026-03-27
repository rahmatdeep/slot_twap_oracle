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

export const MIN_SOURCES = parseInt(process.env.MIN_SOURCES || "2", 10);

export const UPDATE_INTERVAL_MS = parseInt(
  process.env.UPDATE_INTERVAL_MS || "30000",
  10
);

export const PAIRS_CONFIG_PATH = process.env.PAIRS_CONFIG_PATH
  || path.resolve(__dirname, "../config/pairs.json");

export interface PairConfig {
  name: string;
  oracle: string;
  baseMint: string;
  quoteMint: string;
  sources: {
    raydium?: string;
    orca?: string;
    meteora?: string;
  };
}

export function loadPairs(): PairConfig[] {
  if (!fs.existsSync(PAIRS_CONFIG_PATH)) {
    throw new Error(`Pairs config not found: ${PAIRS_CONFIG_PATH}`);
  }

  const raw = fs.readFileSync(PAIRS_CONFIG_PATH, "utf-8");
  const pairs: PairConfig[] = JSON.parse(raw);

  if (!Array.isArray(pairs) || pairs.length === 0) {
    throw new Error("Pairs config must be a non-empty JSON array");
  }

  for (const pair of pairs) {
    if (!pair.name || !pair.oracle || !pair.baseMint || !pair.quoteMint || !pair.sources) {
      throw new Error(
        `Invalid pair config: each entry must have name, oracle, baseMint, quoteMint, and sources`
      );
    }
  }

  return pairs;
}

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
