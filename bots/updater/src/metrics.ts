import fs from "fs";
import path from "path";

const DEFAULT_METRICS_PATH = path.resolve(__dirname, "../data/metrics.json");

export interface MetricsData {
  successful: number;
  failed: number;
  skipped: number;
  lastSuccessfulUpdate: string | null; // ISO timestamp
  lastUpdateSlot: Record<string, number>;
}

function empty(): MetricsData {
  return {
    successful: 0,
    failed: 0,
    skipped: 0,
    lastSuccessfulUpdate: null,
    lastUpdateSlot: {},
  };
}

export class PersistentMetrics {
  private data: MetricsData;
  private filePath: string;

  constructor(filePath?: string) {
    this.filePath = filePath ?? process.env.METRICS_PATH ?? DEFAULT_METRICS_PATH;
    this.data = this.load();
  }

  private load(): MetricsData {
    try {
      if (fs.existsSync(this.filePath)) {
        const raw = fs.readFileSync(this.filePath, "utf-8");
        const parsed = JSON.parse(raw);
        return { ...empty(), ...parsed };
      }
    } catch {
      // Corrupted file — start fresh
    }
    return empty();
  }

  private save(): void {
    const dir = path.dirname(this.filePath);
    if (!fs.existsSync(dir)) {
      fs.mkdirSync(dir, { recursive: true });
    }
    fs.writeFileSync(this.filePath, JSON.stringify(this.data, null, 2) + "\n");
  }

  get successful(): number { return this.data.successful; }
  get failed(): number { return this.data.failed; }
  get skipped(): number { return this.data.skipped; }
  get lastSuccessfulUpdate(): string | null { return this.data.lastSuccessfulUpdate; }

  getLastUpdateSlot(pair: string): number | undefined {
    return this.data.lastUpdateSlot[pair];
  }

  recordSuccess(pair: string, slot: number): void {
    this.data.successful++;
    this.data.lastSuccessfulUpdate = new Date().toISOString();
    this.data.lastUpdateSlot[pair] = slot;
    this.save();
  }

  recordFailure(): void {
    this.data.failed++;
    this.save();
  }

  recordSkip(): void {
    this.data.skipped++;
    this.save();
  }

  log(ts: string): void {
    console.log(
      `${ts} [metrics] successful=${this.data.successful} failed=${this.data.failed} ` +
        `skipped=${this.data.skipped} last_success=${this.data.lastSuccessfulUpdate ?? "never"}`
    );
    for (const [name, slot] of Object.entries(this.data.lastUpdateSlot)) {
      console.log(`${ts} [metrics]   ${name}: last_update_slot=${slot}`);
    }
  }

  snapshot(): MetricsData {
    return { ...this.data, lastUpdateSlot: { ...this.data.lastUpdateSlot } };
  }
}
