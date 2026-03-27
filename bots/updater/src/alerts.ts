import https from "https";
import { URL } from "url";

const RATE_LIMIT_MS = 5 * 60 * 1000; // 5 minutes
let lastAlertTime = 0;

function isRateLimited(): boolean {
  const now = Date.now();
  if (now - lastAlertTime < RATE_LIMIT_MS) return true;
  lastAlertTime = now;
  return false;
}

/**
 * Send a Telegram message via the Bot API.
 * Set TELEGRAM_BOT_TOKEN and TELEGRAM_CHAT_ID in the environment to enable.
 */
export async function alertTelegram(message: string): Promise<void> {
  const botToken = process.env.TELEGRAM_BOT_TOKEN;
  const chatId = process.env.TELEGRAM_CHAT_ID;
  if (!botToken || !chatId) return;

  const url = new URL(`https://api.telegram.org/bot${botToken}/sendMessage`);
  const body = JSON.stringify({
    chat_id: chatId,
    text: message,
    parse_mode: "Markdown",
  });

  return new Promise((resolve, reject) => {
    const req = https.request(
      url,
      {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          "Content-Length": Buffer.byteLength(body),
        },
      },
      (res) => {
        res.resume();
        if (res.statusCode && res.statusCode >= 200 && res.statusCode < 300) {
          resolve();
        } else {
          reject(new Error(`Telegram API returned ${res.statusCode}`));
        }
      }
    );
    req.on("error", reject);
    req.write(body);
    req.end();
  });
}

export interface StaleOracleInfo {
  name: string;
  lastSlot: number;
  currentSlot: number;
}

/**
 * Alert via Telegram if any oracles are stale.
 * Rate-limited to once per 5 minutes.
 */
export async function alertIfStale(
  staleOracles: StaleOracleInfo[]
): Promise<void> {
  if (staleOracles.length === 0) return;
  if (isRateLimited()) return;

  const details = staleOracles
    .map((o) => `  • *${o.name}*: last slot ${o.lastSlot} (${o.currentSlot - o.lastSlot} behind)`)
    .join("\n");

  const msg =
    `🚨 *Oracle Staleness Alert*\n` +
    `${staleOracles.length} oracle(s) stale:\n${details}`;

  try {
    await alertTelegram(msg);
  } catch (err) {
    console.error(
      `${new Date().toISOString()} [alerts] Failed to send Telegram alert: ${(err as Error).message}`
    );
  }
}
