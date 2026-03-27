import { PublicKey } from "@solana/web3.js";
import BN from "bn.js";

export interface OracleAccount {
  owner: PublicKey;
  baseMint: PublicKey;
  quoteMint: PublicKey;
  lastPrice: BN;
  cumulativePrice: BN;
  lastSlot: BN;
  lastUpdater: PublicKey;
  paused: boolean;
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

export interface OracleUpdateEvent {
  oracle: PublicKey;
  price: BN;
  cumulativePrice: BN;
  slot: BN;
  updater: PublicKey;
}
