import { Program, AnchorProvider, BN } from "@coral-xyz/anchor";
import { PublicKey, Signer } from "@solana/web3.js";
import { IDL, SlotTwapOracle } from "./idl";
import { findOraclePda, findObservationBufferPda, PROGRAM_ID } from "./pda";
import {
  OracleAccount,
  ObservationBufferAccount,
  Observation,
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
    authority: Signer
  ): Promise<string> {
    const [observationBuffer] = this.findObservationBufferPda(oracle);

    return this.program.methods
      .updatePrice(newPrice)
      .accounts({
        authority: authority.publicKey,
        oracle,
        observationBuffer,
      })
      .signers([authority])
      .rpc();
  }

  async getSwap(oracle: PublicKey, windowSlots: BN): Promise<BN> {
    const [observationBuffer] = this.findObservationBufferPda(oracle);

    const result = await this.program.methods
      .getSwap(windowSlots)
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
    const len = buffer.observations.length;
    if (len === 0) return null;

    for (let i = 1; i <= len; i++) {
      const idx = (buffer.head + len - i) % len;
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
}
