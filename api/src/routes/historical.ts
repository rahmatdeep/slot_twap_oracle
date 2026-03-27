import { Router, Request, Response } from "express";
import { getPool } from "../db";
import { historicalQuery, validateQuery } from "../validate";

const router = Router();

/**
 * GET /historical?oracle=<pk>&from_slot=N&to_slot=M&interval=100&limit=200
 *
 * Returns historical price data from the PostgreSQL indexer, bucketed by
 * slot intervals for charting. Each bucket contains the avg/min/max price
 * and the slot range.
 *
 * Requires POSTGRES_URL to be set. Returns 503 if not configured.
 */
router.get("/", validateQuery(historicalQuery), async (_req: Request, res: Response) => {
  const pool = getPool();
  if (!pool) {
    res.status(503).json({ error: "Historical data not available — POSTGRES_URL not configured" });
    return;
  }

  try {
    const { oracle, from_slot, to_slot, interval, limit } = res.locals.query;

    // Build query with optional slot range
    const conditions = ["oracle_pubkey = $1"];
    const params: (string | number)[] = [oracle];
    let paramIdx = 2;

    if (from_slot !== undefined) {
      conditions.push(`slot >= $${paramIdx}`);
      params.push(from_slot);
      paramIdx++;
    }
    if (to_slot !== undefined) {
      conditions.push(`slot <= $${paramIdx}`);
      params.push(to_slot);
      paramIdx++;
    }

    const where = conditions.join(" AND ");

    // Bucket by slot interval, compute aggregate price stats per bucket
    const query = `
      SELECT
        (slot / $${paramIdx})::bigint * $${paramIdx} AS bucket_slot,
        COUNT(*)::int AS update_count,
        AVG(price::numeric)::numeric AS avg_price,
        MIN(price::numeric)::numeric AS min_price,
        MAX(price::numeric)::numeric AS max_price,
        MIN(slot) AS first_slot,
        MAX(slot) AS last_slot
      FROM oracle_updates
      WHERE ${where}
      GROUP BY bucket_slot
      ORDER BY bucket_slot DESC
      LIMIT $${paramIdx + 1}
    `;
    params.push(interval, limit);

    const result = await pool.query(query, params);

    res.json({
      oracle,
      interval,
      count: result.rows.length,
      buckets: result.rows.map((r: any) => ({
        slot: Number(r.bucket_slot),
        updateCount: r.update_count,
        avgPrice: r.avg_price,
        minPrice: r.min_price,
        maxPrice: r.max_price,
        firstSlot: Number(r.first_slot),
        lastSlot: Number(r.last_slot),
      })),
    });
  } catch (err) {
    res.status(500).json({ error: (err as Error).message });
  }
});

export default router;
