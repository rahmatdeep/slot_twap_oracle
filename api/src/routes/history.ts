import { Router, Request, Response } from "express";
import { PublicKey } from "@solana/web3.js";
import { client } from "../client";
import { historyQuery, validateQuery } from "../validate";

const router = Router();

router.get("/", validateQuery(historyQuery), async (_req: Request, res: Response) => {
  try {
    const { oracle, limit } = res.locals.query;
    const oraclePubkey = new PublicKey(oracle);

    const events = await client.getOracleUpdates(oraclePubkey, limit);

    res.json({
      oracle,
      count: events.length,
      updates: events.map((e) => ({
        oracle: e.oracle.toBase58(),
        price: e.price.toString(),
        cumulativePrice: e.cumulativePrice.toString(),
        slot: e.slot.toString(),
        updater: e.updater.toBase58(),
      })),
    });
  } catch (err) {
    res.status(500).json({ error: (err as Error).message });
  }
});

export default router;
