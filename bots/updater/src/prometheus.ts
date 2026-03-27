import http from "http";
import { PersistentMetrics } from "./metrics";

/**
 * Starts an HTTP server that exposes Prometheus-compatible metrics at /metrics.
 */
export function startPrometheusServer(
  metrics: PersistentMetrics,
  port: number = 9090
): http.Server {
  const server = http.createServer((_req, res) => {
    if (_req.url !== "/metrics") {
      res.writeHead(404);
      res.end();
      return;
    }

    const snap = metrics.snapshot();
    const lines: string[] = [];

    lines.push("# HELP updates_successful_total Total successful price updates");
    lines.push("# TYPE updates_successful_total counter");
    lines.push(`updates_successful_total ${snap.successful}`);

    lines.push("# HELP updates_failed_total Total failed price updates");
    lines.push("# TYPE updates_failed_total counter");
    lines.push(`updates_failed_total ${snap.failed}`);

    lines.push("# HELP updates_skipped_total Total skipped price updates");
    lines.push("# TYPE updates_skipped_total counter");
    lines.push(`updates_skipped_total ${snap.skipped}`);

    lines.push("# HELP stale_oracles_total Total stale oracle detections");
    lines.push("# TYPE stale_oracles_total counter");
    lines.push(`stale_oracles_total ${snap.staleOracles}`);

    lines.push("# HELP oracle_last_update_slot Last successful update slot per oracle pair");
    lines.push("# TYPE oracle_last_update_slot gauge");
    for (const [pair, slot] of Object.entries(snap.lastUpdateSlot)) {
      const label = pair.replace(/"/g, '\\"');
      lines.push(`oracle_last_update_slot{pair="${label}"} ${slot}`);
    }

    res.writeHead(200, { "Content-Type": "text/plain; version=0.0.4; charset=utf-8" });
    res.end(lines.join("\n") + "\n");
  });

  server.listen(port, () => {
    console.log(`${new Date().toISOString()} [prometheus] Metrics server on :${port}/metrics`);
  });

  return server;
}
