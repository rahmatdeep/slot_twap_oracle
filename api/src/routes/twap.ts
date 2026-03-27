import { Router, Request, Response } from "express";
import { PublicKey } from "@solana/web3.js";
import { client, connection } from "../client";
import { twapQuery, validateQuery } from "../validate";

const router = Router();

router.get("/", validateQuery(twapQuery), async (_req: Request, res: Response) => {
  try {
    const { oracle, window } = res.locals.query;
    const oraclePubkey = new PublicKey(oracle);

    const oracleAccount = await client.fetchOracle(oraclePubkey);
    const twap = await client.computeSwapFromChain(
      oracleAccount.baseMint,
      oracleAccount.quoteMint,
      window
    );
    const currentSlot = await connection.getSlot();

    res.json({
      oracle,
      twap: twap.toString(),
      windowSlots: window,
      currentSlot,
    });
  } catch (err) {
    const msg = (err as Error).message;
    const status = msg.includes("Insufficient") ? 422 : 500;
    res.status(status).json({ error: msg });
  }
});

export default router;
