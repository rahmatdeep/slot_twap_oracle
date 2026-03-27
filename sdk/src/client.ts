import { Program, AnchorProvider, BN } from "@coral-xyz/anchor";
import { PublicKey, Signer } from "@solana/web3.js";
import { IDL, SlotTwapOracle } from "./idl";
import { findOraclePda, findObservationBufferPda, PROGRAM_ID } from "./pda";
import {
  OracleAccount,
  ObservationBufferAccount,
  Observation,
  OracleUpdateEvent,
} from "./types";

/**
 * Client for the Slot TWAP Oracle program.
 *
 * @param provider  - Anchor provider with connection and wallet.
 * @param programId - Optional program ID override. Defaults to the canonical
 *                    deployed program ID ({@link PROGRAM_ID}). Pass a custom
 *                    value when targeting localnet or a custom deployment.
 */
export class SlotTwapOracleClient {
  readonly program: Program<SlotTwapOracle>;
  readonly programId: PublicKey;

  constructor(provider: AnchorProvider, programId: PublicKey = PROGRAM_ID) {
    this.programId = programId;
    this.program = new Program(IDL, provider) as Program<SlotTwapOracle>;
  }

  // ── PDA helpers ──
  //
  // These use the programId passed to the constructor, so PDAs match the
  // target deployment even when overriding the default program ID.

  findOraclePda(baseMint: PublicKey, quoteMint: PublicKey): [PublicKey, number] {
    return findOraclePda(baseMint, quoteMint, this.programId);
  }

  findObservationBufferPda(oracle: PublicKey): [PublicKey, number] {
    return findObservationBufferPda(oracle, this.programId);
  }

  // ── Instructions ──

  async initializeOracle(
    baseMint: PublicKey,
    quoteMint: PublicKey,
    capacity: number,
    authority: Signer
  ): Promise<string> {
    return this.program.methods
      .initializeOracle(capacity)
      .accounts({
        baseMint,
        quoteMint,
        authority: authority.publicKey,
      })
      .signers([authority])
      .rpc();
  }

  async updatePrice(
    oracle: PublicKey,
    newPrice: BN,
    payer: Signer
  ): Promise<string> {
    return this.program.methods
      .updatePrice(newPrice)
      .accounts({
        payer: payer.publicKey,
        oracle,
      })
      .signers([payer])
      .rpc();
  }

  async transferOwnership(
    oracle: PublicKey,
    newOwner: PublicKey,
    owner: Signer
  ): Promise<string> {
    return this.program.methods
      .transferOwnership()
      .accounts({
        oracle,
        owner: owner.publicKey,
        newOwner,
      })
      .signers([owner])
      .rpc();
  }

  async setPaused(
    oracle: PublicKey,
    paused: boolean,
    owner: Signer
  ): Promise<string> {
    return this.program.methods
      .setPaused(paused)
      .accounts({
        oracle,
        owner: owner.publicKey,
      })
      .signers([owner])
      .rpc();
  }

  async setMaxDeviation(
    oracle: PublicKey,
    newMaxDeviationBps: number,
    owner: Signer
  ): Promise<string> {
    return this.program.methods
      .setMaxDeviation(newMaxDeviationBps)
      .accounts({
        oracle,
        owner: owner.publicKey,
      })
      .signers([owner])
      .rpc();
  }

  async resizeBuffer(
    oracle: PublicKey,
    newCapacity: number,
    owner: Signer
  ): Promise<string> {
    const [observationBuffer] = this.findObservationBufferPda(oracle);

    return this.program.methods
      .resizeBuffer(newCapacity)
      .accounts({
        oracle,
        observationBuffer,
        owner: owner.publicKey,
      })
      .signers([owner])
      .rpc();
  }

  async getSwap(oracle: PublicKey, windowSlots: BN, maxStalenessSlots: BN): Promise<BN> {
    const [observationBuffer] = this.findObservationBufferPda(oracle);

    const result = await this.program.methods
      .getSwap(windowSlots, maxStalenessSlots)
      .accounts({
        oracle,
        observationBuffer,
      })
      .view();

    return result as BN;
  }

  // ── Account fetchers ──

  async fetchOracle(address: PublicKey): Promise<OracleAccount> {
    const account = await this.program.account.oracle.fetch(address);
    return account as unknown as OracleAccount;
  }

  async fetchObservationBuffer(
    address: PublicKey
  ): Promise<ObservationBufferAccount> {
    const account =
      await this.program.account.observationBuffer.fetch(address);
    return account as unknown as ObservationBufferAccount;
  }

  async fetchOracleByPair(
    baseMint: PublicKey,
    quoteMint: PublicKey
  ): Promise<OracleAccount> {
    const [oraclePda] = this.findOraclePda(baseMint, quoteMint);
    return this.fetchOracle(oraclePda);
  }

  // ── Utility ──

  /**
   * Compute TWAP off-chain from two observation snapshots.
   * Returns the slot-weighted average price as a BN.
   */
  static computeSwap(
    cumulativeNow: BN,
    cumulativePast: BN,
    slotNow: BN,
    slotPast: BN
  ): BN {
    const slotDelta = slotNow.sub(slotPast);
    if (slotDelta.isZero()) {
      throw new Error("Division by zero: slot span is zero");
    }
    return cumulativeNow.sub(cumulativePast).div(slotDelta);
  }

  /**
   * Find the most recent observation with slot < targetSlot.
   * Scans backwards from head (most recent first).
   */
  static findObservationBeforeSlot(
    buffer: ObservationBufferAccount,
    targetSlot: BN
  ): Observation | null {
    const populated = buffer.len;
    if (populated === 0) return null;

    const cap = buffer.capacity;
    for (let i = 1; i <= populated; i++) {
      const idx = (buffer.head + cap - i) % cap;
      const obs = buffer.observations[idx];
      if (obs.slot.lt(targetSlot)) {
        return obs;
      }
    }

    return null;
  }

  /**
   * Compute TWAP for a pair over a window, fetching state on-chain.
   * Extends cumulative price to the current slot like get_swap does.
   */
  async computeSwapFromChain(
    baseMint: PublicKey,
    quoteMint: PublicKey,
    windowSlots: number
  ): Promise<BN> {
    const [oraclePda] = this.findOraclePda(baseMint, quoteMint);
    const [bufferPda] = this.findObservationBufferPda(oraclePda);

    const [oracle, buffer, slot] = await Promise.all([
      this.fetchOracle(oraclePda),
      this.fetchObservationBuffer(bufferPda),
      this.program.provider.connection.getSlot(),
    ]);

    const currentSlot = new BN(slot);
    const slotDeltaSinceLast = currentSlot.sub(oracle.lastSlot);
    const cumulativeNow = oracle.cumulativePrice.add(
      oracle.lastPrice.mul(slotDeltaSinceLast)
    );

    const windowStart = currentSlot.sub(new BN(windowSlots));
    const pastObs = SlotTwapOracleClient.findObservationBeforeSlot(
      buffer,
      windowStart.add(new BN(1))
    );

    if (!pastObs) {
      throw new Error("Insufficient observations for the requested window");
    }

    return SlotTwapOracleClient.computeSwap(
      cumulativeNow,
      pastObs.cumulativePrice,
      currentSlot,
      pastObs.slot
    );
  }

  // ── Event parsing ──

  /**
   * Parse OracleUpdate events from a confirmed transaction's logs.
   */
  async parseOracleUpdateEvents(
    txSignature: string
  ): Promise<OracleUpdateEvent[]> {
    const tx = await this.program.provider.connection.getTransaction(
      txSignature,
      { maxSupportedTransactionVersion: 0 }
    );
    if (!tx?.meta?.logMessages) return [];

    return SlotTwapOracleClient.decodeOracleUpdateLogs(tx.meta.logMessages);
  }

  /**
   * Fetch recent OracleUpdate events for a given oracle address.
   * Scans the last `limit` transactions (default 20) involving the oracle account.
   */
  async getOracleUpdates(
    oraclePubkey: PublicKey,
    limit: number = 20
  ): Promise<OracleUpdateEvent[]> {
    const conn = this.program.provider.connection;
    const signatures = await conn.getSignaturesForAddress(oraclePubkey, {
      limit,
    });

    if (signatures.length === 0) return [];

    const txs = await conn.getTransactions(
      signatures.map((s) => s.signature),
      { maxSupportedTransactionVersion: 0 }
    );

    const events: OracleUpdateEvent[] = [];
    for (const tx of txs) {
      if (!tx?.meta?.logMessages) continue;
      events.push(
        ...SlotTwapOracleClient.decodeOracleUpdateLogs(tx.meta.logMessages)
      );
    }

    return events;
  }

  /**
   * Decode OracleUpdate events from raw program log lines.
   * Anchor emits events via sol_log_data, surfaced as "Program data: <base64>".
   */
  static decodeOracleUpdateLogs(logs: string[]): OracleUpdateEvent[] {
    const EVENT_DISCRIMINATOR = Buffer.from([237, 176, 133, 150, 0, 131, 48, 15]);
    const events: OracleUpdateEvent[] = [];

    for (const line of logs) {
      if (!line.startsWith("Program data: ")) continue;

      const b64 = line.slice("Program data: ".length);
      let data: Buffer;
      try {
        data = Buffer.from(b64, "base64");
      } catch {
        continue;
      }

      if (data.length < 8 || !data.subarray(0, 8).equals(EVENT_DISCRIMINATOR)) {
        continue;
      }

      // Layout after discriminator: oracle (32) + price (16) + cumulative_price (16) + slot (8) + updater (32)
      const payload = data.subarray(8);
      if (payload.length < 104) continue;

      let offset = 0;
      const oracle = new PublicKey(payload.subarray(offset, offset + 32));
      offset += 32;

      const price = new BN(payload.subarray(offset, offset + 16), "le");
      offset += 16;

      const cumulativePrice = new BN(payload.subarray(offset, offset + 16), "le");
      offset += 16;

      const slot = new BN(payload.subarray(offset, offset + 8), "le");
      offset += 8;

      const updater = new PublicKey(payload.subarray(offset, offset + 32));

      events.push({ oracle, price, cumulativePrice, slot, updater });
    }

    return events;
  }
}
