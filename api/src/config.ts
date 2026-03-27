import { z } from "zod";

const envSchema = z.object({
  RPC_URL: z.string().url().default("http://127.0.0.1:8899"),
  PORT: z.coerce.number().int().positive().default(3000),
  PROGRAM_ID: z.string().optional(),
  RATE_LIMIT_WINDOW_MS: z.coerce.number().int().positive().default(60_000),
  RATE_LIMIT_MAX: z.coerce.number().int().positive().default(60),
  WS_MAX_CONNECTIONS: z.coerce.number().int().positive().default(100),
  WS_MAX_SUBS_PER_CLIENT: z.coerce.number().int().positive().default(10),
  WS_MSG_PER_MIN: z.coerce.number().int().positive().default(30),
});

const parsed = envSchema.safeParse(process.env);
if (!parsed.success) {
  console.error("Invalid environment variables:");
  console.error(parsed.error.format());
  process.exit(1);
}

export const config = parsed.data;
