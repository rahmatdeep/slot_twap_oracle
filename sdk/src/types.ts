import { PublicKey } from "@solana/web3.js";
import BN from "bn.js";

export interface OracleAccount {
  authority: PublicKey;
  baseMint: PublicKey;
  quoteMint: PublicKey;
  lastPrice: BN;
  cumulativePrice: BN;
  lastSlot: BN;
}

export interface Observation {
  slot: BN;
  cumulativePrice: BN;
}

export interface ObservationBufferAccount {
  oracle: PublicKey;
  head: number;
  capacity: number;
  observations: Observation[];
}

export interface PriceUpdatedEvent {
  slot: BN;
  newPrice: BN;
  cumulativePrice: BN;
}
