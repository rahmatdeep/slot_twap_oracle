import { Router, Request, Response } from "express";
import { connection } from "../client";

const router = Router();

router.get("/", async (_req: Request, res: Response) => {
  try {
    const slot = await connection.getSlot();
    res.json({ status: "ok", slot });
  } catch (err) {
    res.status(503).json({ status: "unhealthy", error: (err as Error).message });
  }
});

export default router;
