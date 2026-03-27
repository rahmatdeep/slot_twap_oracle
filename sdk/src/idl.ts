// Auto-synced from target/idl/slot_twap_oracle.json via scripts/sync-idl.sh
// Do not edit manually — run `anchor build && bash scripts/sync-idl.sh`

import { Idl } from "@coral-xyz/anchor";
import idlJson from "./idl.json";

// Cast to Idl — the generated JSON is always structurally compatible.
// We use a loose cast because JSON imports widen string literals ("const" -> string).
export const IDL: Idl = idlJson as unknown as Idl;
export type SlotTwapOracle = Idl;
