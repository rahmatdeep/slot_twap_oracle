import { Pool } from "pg";
import { config } from "./config";

let pool: Pool | null = null;

export function getPool(): Pool | null {
  if (!config.POSTGRES_URL) return null;
  if (!pool) {
    pool = new Pool({ connectionString: config.POSTGRES_URL });
  }
  return pool;
}

export function isDbAvailable(): boolean {
  return !!config.POSTGRES_URL;
}
