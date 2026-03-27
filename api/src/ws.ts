import { Server as HttpServer } from "http";
import { WebSocketServer, WebSocket } from "ws";
import { PublicKey } from "@solana/web3.js";
import BN from "bn.js";
import { client, connection } from "./client";
import { config } from "./config";

const POLL_INTERVAL_MS = parseInt(process.env.WS_POLL_MS || "2000", 10);
const MAX_CONNECTIONS = config.WS_MAX_CONNECTIONS;
const MAX_SUBS_PER_CLIENT = config.WS_MAX_SUBS_PER_CLIENT;
const MSG_PER_MIN = config.WS_MSG_PER_MIN;

interface Subscription {
  oracle: PublicKey;
  windowSlots: number;
}

interface ClientState {
  ws: WebSocket;
  subscriptions: Map<string, Subscription>;
  msgCount: number;
  msgWindowStart: number;
}

const clients = new Set<ClientState>();
let pollTimer: ReturnType<typeof setInterval> | null = null;

function ts(): string {
  return new Date().toISOString();
}

function send(ws: WebSocket, data: object): void {
  if (ws.readyState === WebSocket.OPEN) {
    ws.send(JSON.stringify(data));
  }
}

function isRateLimited(state: ClientState): boolean {
  const now = Date.now();
  if (now - state.msgWindowStart > 60_000) {
    state.msgCount = 0;
    state.msgWindowStart = now;
  }
  state.msgCount++;
  return state.msgCount > MSG_PER_MIN;
}

function handleMessage(state: ClientState, raw: string): void {
  if (isRateLimited(state)) {
    send(state.ws, { error: "Rate limited — too many messages" });
    return;
  }

  let msg: any;
  try {
    msg = JSON.parse(raw);
  } catch {
    send(state.ws, { error: "Invalid JSON" });
    return;
  }

  if (msg.action === "subscribe") {
    const { oracle, window } = msg;
    if (!oracle || !window) {
      send(state.ws, { error: "subscribe requires oracle and window" });
      return;
    }

    if (state.subscriptions.size >= MAX_SUBS_PER_CLIENT) {
      send(state.ws, {
        error: `Subscription limit reached (max ${MAX_SUBS_PER_CLIENT})`,
      });
      return;
    }

    let oraclePubkey: PublicKey;
    try {
      oraclePubkey = new PublicKey(oracle);
    } catch {
      send(state.ws, { error: `Invalid oracle pubkey: ${oracle}` });
      return;
    }

    const windowSlots = parseInt(window, 10);
    if (isNaN(windowSlots) || windowSlots <= 0) {
      send(state.ws, { error: "window must be a positive integer" });
      return;
    }

    const key = `${oracle}:${windowSlots}`;
    state.subscriptions.set(key, { oracle: oraclePubkey, windowSlots });
    send(state.ws, { type: "subscribed", oracle, window: windowSlots });
    console.log(`${ts()} [ws] Client subscribed: ${oracle} window=${windowSlots}`);
  } else if (msg.action === "unsubscribe") {
    const { oracle, window } = msg;
    const key = `${oracle}:${window}`;
    state.subscriptions.delete(key);
    send(state.ws, { type: "unsubscribed", oracle, window });
  } else {
    send(state.ws, { error: `Unknown action: ${msg.action}` });
  }
}

async function broadcastUpdates(): Promise<void> {
  if (clients.size === 0) return;

  // Collect all unique subscriptions across clients
  const uniqueSubs = new Map<string, Subscription>();
  for (const state of clients) {
    for (const [key, sub] of state.subscriptions) {
      uniqueSubs.set(key, sub);
    }
  }
  if (uniqueSubs.size === 0) return;

  // Fetch data for each unique subscription
  const results = new Map<string, object>();

  for (const [key, sub] of uniqueSubs) {
    try {
      const oracleAccount = await client.fetchOracle(sub.oracle);
      const currentSlot = await connection.getSlot();
      const slotDelta = currentSlot - oracleAccount.lastSlot.toNumber();

      const twap = await client.computeSwapFromChain(
        oracleAccount.baseMint,
        oracleAccount.quoteMint,
        sub.windowSlots
      );

      results.set(key, {
        type: "update",
        oracle: sub.oracle.toBase58(),
        window: sub.windowSlots,
        twap: twap.toString(),
        price: oracleAccount.lastPrice.toString(),
        slot: oracleAccount.lastSlot.toString(),
        updater: oracleAccount.lastUpdater.toBase58(),
        paused: oracleAccount.paused,
        currentSlot,
        slotsSinceUpdate: slotDelta,
        timestamp: ts(),
      });
    } catch (err) {
      results.set(key, {
        type: "error",
        oracle: sub.oracle.toBase58(),
        window: sub.windowSlots,
        error: (err as Error).message,
        timestamp: ts(),
      });
    }
  }

  // Send to each client their subscribed data
  for (const state of clients) {
    for (const [key] of state.subscriptions) {
      const data = results.get(key);
      if (data) send(state.ws, data);
    }
  }
}

export function attachWebSocket(server: HttpServer): WebSocketServer {
  const wss = new WebSocketServer({ server, path: "/ws" });

  wss.on("connection", (ws) => {
    if (clients.size >= MAX_CONNECTIONS) {
      ws.close(1013, "Maximum connections reached");
      console.log(`${ts()} [ws] Rejected connection (${clients.size}/${MAX_CONNECTIONS})`);
      return;
    }

    const state: ClientState = {
      ws,
      subscriptions: new Map(),
      msgCount: 0,
      msgWindowStart: Date.now(),
    };
    clients.add(state);
    console.log(`${ts()} [ws] Client connected (${clients.size}/${MAX_CONNECTIONS})`);

    send(ws, { type: "connected", message: "Send {action:'subscribe', oracle:'...', window:N}" });

    ws.on("message", (data) => handleMessage(state, data.toString()));

    ws.on("close", () => {
      clients.delete(state);
      if (!stopping) {
        console.log(`${ts()} [ws] Client disconnected (${clients.size}/${MAX_CONNECTIONS})`);
      }
    });

    ws.on("error", (err) => {
      console.error(`${ts()} [ws] Error: ${err.message}`);
    });
  });

  // Start polling for updates
  if (!pollTimer) {
    pollTimer = setInterval(broadcastUpdates, POLL_INTERVAL_MS);
    console.log(`${ts()} [ws] Broadcasting updates every ${POLL_INTERVAL_MS}ms on /ws`);
  }

  return wss;
}

let stopping = false;

export function stopBroadcast(): void {
  stopping = true;
  if (pollTimer) {
    clearInterval(pollTimer);
    pollTimer = null;
  }
  for (const state of clients) {
    state.ws.close();
  }
  clients.clear();
}
