import { z } from "zod";
import { PublicKey } from "@solana/web3.js";
import { Request, Response, NextFunction } from "express";

const pubkey = z.string().refine(
  (val) => {
    try { new PublicKey(val); return true; } catch { return false; }
  },
  { message: "Invalid Solana pubkey" }
);

export const priceQuery = z.object({
  oracle: pubkey,
});

export const twapQuery = z.object({
  oracle: pubkey,
  window: z.coerce.number().int().positive({ message: "window must be a positive integer" }),
});

export const historyQuery = z.object({
  oracle: pubkey,
  limit: z.coerce.number().int().positive().max(100).default(20),
});

export const historicalQuery = z.object({
  oracle: pubkey,
  from_slot: z.coerce.number().int().nonnegative().optional(),
  to_slot: z.coerce.number().int().positive().optional(),
  interval: z.coerce.number().int().positive().default(100),
  limit: z.coerce.number().int().positive().max(1000).default(200),
});

/**
 * Express middleware that validates req.query against a Zod schema.
 * On success, attaches parsed data to res.locals.query.
 */
export function validateQuery<T extends z.ZodTypeAny>(schema: T) {
  return (req: Request, res: Response, next: NextFunction): void => {
    const result = schema.safeParse(req.query);
    if (!result.success) {
      const errors = result.error.issues.map((i) => ({
        field: i.path.join("."),
        message: i.message,
      }));
      res.status(400).json({ error: "Validation failed", details: errors });
      return;
    }
    res.locals.query = result.data;
    next();
  };
}
