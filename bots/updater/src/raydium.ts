import { Connection, PublicKey } from "@solana/web3.js";
import BN from "bn.js";

// Raydium AMM v4 account layout offsets for token vault pubkeys.
// The full layout is 752 bytes; we only need the two vault addresses.
const BASE_VAULT_OFFSET = 336;
const QUOTE_VAULT_OFFSET = 368;

/**
 * Reads the base and quote token vault addresses from a Raydium AMM account,
 * fetches their token balances, and computes price = quote / base.
 *
 * Returns price scaled to an integer (multiplied by 10^PRICE_DECIMALS)
 * so it can be stored as u128 on-chain.
 */
export const PRICE_DECIMALS = 9;
const SCALE = new BN(10).pow(new BN(PRICE_DECIMALS));

export async function fetchRaydiumPrice(
  connection: Connection,
  ammId: PublicKey
): Promise<bigint> {
  const ammAccount = await connection.getAccountInfo(ammId);
  if (!ammAccount) throw new Error(`AMM account not found: ${ammId.toBase58()}`);

  const data = ammAccount.data;

  const baseVault = new PublicKey(data.subarray(BASE_VAULT_OFFSET, BASE_VAULT_OFFSET + 32));
  const quoteVault = new PublicKey(data.subarray(QUOTE_VAULT_OFFSET, QUOTE_VAULT_OFFSET + 32));

  const [baseBalance, quoteBalance] = await Promise.all([
    connection.getTokenAccountBalance(baseVault),
    connection.getTokenAccountBalance(quoteVault),
  ]);

  const baseAmount = new BN(baseBalance.value.amount);
  const quoteAmount = new BN(quoteBalance.value.amount);

  if (baseAmount.isZero()) {
    throw new Error("Base reserve is zero — cannot compute price");
  }

  // price = (quoteAmount * SCALE) / baseAmount
  const scaledPrice = quoteAmount.mul(SCALE).div(baseAmount);

  return BigInt(scaledPrice.toString());
}
