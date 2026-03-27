import { Router, Request, Response } from "express";
import { PublicKey } from "@solana/web3.js";
import { client, connection } from "../client";
import { priceQuery, validateQuery } from "../validate";

const router = Router();

router.get("/", validateQuery(priceQuery), async (_req: Request, res: Response) => {
  try {
    const { oracle } = res.locals.query;
    const oraclePubkey = new PublicKey(oracle);

    const [oracleAccount, currentSlot] = await Promise.all([
      client.fetchOracle(oraclePubkey),
      connection.getSlot(),
    ]);

    res.json({
      oracle,
      baseMint: oracleAccount.baseMint.toBase58(),
      quoteMint: oracleAccount.quoteMint.toBase58(),
      price: oracleAccount.lastPrice.toString(),
      cumulativePrice: oracleAccount.cumulativePrice.toString(),
      slot: oracleAccount.lastSlot.toString(),
      updater: oracleAccount.lastUpdater.toBase58(),
      owner: oracleAccount.owner.toBase58(),
      paused: oracleAccount.paused,
      maxDeviationBps: oracleAccount.maxDeviationBps,
      currentSlot,
      slotsSinceUpdate: currentSlot - oracleAccount.lastSlot.toNumber(),
    });
  } catch (err) {
    res.status(500).json({ error: (err as Error).message });
  }
});

export default router;
