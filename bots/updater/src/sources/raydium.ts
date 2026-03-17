import { Connection, PublicKey } from "@solana/web3.js";
import BN from "bn.js";

const RAYDIUM_AMM_PROGRAM_ID = new PublicKey(
  "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8"
);

// Raydium AMM v4 account layout offsets for token vault pubkeys.
const AMM_ACCOUNT_MIN_SIZE = 752;
const BASE_VAULT_OFFSET = 336;
const QUOTE_VAULT_OFFSET = 368;

/**
 * Fetches the spot price from a Raydium AMM v4 pool.
 * Returns price as quote / base (floating point).
 */
export async function fetchPrice(
  connection: Connection,
  poolAddress: PublicKey
): Promise<number> {
  const ammAccount = await connection.getAccountInfo(poolAddress);
  if (!ammAccount)
    throw new Error(`Raydium AMM account not found: ${poolAddress.toBase58()}`);

  if (!ammAccount.owner.equals(RAYDIUM_AMM_PROGRAM_ID)) {
    throw new Error(
      `Raydium AMM account ${poolAddress.toBase58()} not owned by Raydium program`
    );
  }

  if (ammAccount.data.length < AMM_ACCOUNT_MIN_SIZE) {
    throw new Error(
      `Raydium AMM account data too small: expected >= ${AMM_ACCOUNT_MIN_SIZE}, got ${ammAccount.data.length}`
    );
  }

  const data = ammAccount.data;
  const baseVault = new PublicKey(
    data.subarray(BASE_VAULT_OFFSET, BASE_VAULT_OFFSET + 32)
  );
  const quoteVault = new PublicKey(
    data.subarray(QUOTE_VAULT_OFFSET, QUOTE_VAULT_OFFSET + 32)
  );

  const [baseBalance, quoteBalance] = await Promise.all([
    connection.getTokenAccountBalance(baseVault),
    connection.getTokenAccountBalance(quoteVault),
  ]);

  const baseAmount = new BN(baseBalance.value.amount);
  const quoteAmount = new BN(quoteBalance.value.amount);

  if (baseAmount.isZero()) {
    throw new Error("Raydium: base reserve is zero");
  }

  // Compute as float with enough precision via BN intermediate
  const SCALE = new BN(10).pow(new BN(18));
  const scaled = quoteAmount.mul(SCALE).div(baseAmount);
  return Number(scaled.toString()) / 1e18;
}
