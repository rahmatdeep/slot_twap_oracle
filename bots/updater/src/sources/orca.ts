import { Connection, PublicKey } from "@solana/web3.js";
import BN from "bn.js";

const ORCA_WHIRLPOOL_PROGRAM_ID = new PublicKey(
  "whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc"
);

// Orca Whirlpool account layout offsets (after 8-byte Anchor discriminator).
// token_vault_a: Pubkey at offset 165, token_vault_b: Pubkey at offset 197.
const WHIRLPOOL_MIN_SIZE = 229;
const VAULT_A_OFFSET = 165;
const VAULT_B_OFFSET = 197;

/**
 * Fetches the spot price from an Orca Whirlpool.
 * Reads vault token balances and returns price as quote / base (floating point).
 *
 * Assumes token A is the base token and token B is the quote token,
 * matching the Whirlpool's canonical token ordering.
 */
export async function fetchPrice(
  connection: Connection,
  poolAddress: PublicKey
): Promise<number> {
  const poolAccount = await connection.getAccountInfo(poolAddress);
  if (!poolAccount)
    throw new Error(`Orca Whirlpool not found: ${poolAddress.toBase58()}`);

  if (!poolAccount.owner.equals(ORCA_WHIRLPOOL_PROGRAM_ID)) {
    throw new Error(
      `Orca Whirlpool ${poolAddress.toBase58()} not owned by Whirlpool program`
    );
  }

  if (poolAccount.data.length < WHIRLPOOL_MIN_SIZE) {
    throw new Error(
      `Orca Whirlpool data too small: expected >= ${WHIRLPOOL_MIN_SIZE}, got ${poolAccount.data.length}`
    );
  }

  const data = poolAccount.data;
  const vaultA = new PublicKey(
    data.subarray(VAULT_A_OFFSET, VAULT_A_OFFSET + 32)
  );
  const vaultB = new PublicKey(
    data.subarray(VAULT_B_OFFSET, VAULT_B_OFFSET + 32)
  );

  const [balanceA, balanceB] = await Promise.all([
    connection.getTokenAccountBalance(vaultA),
    connection.getTokenAccountBalance(vaultB),
  ]);

  const amountA = new BN(balanceA.value.amount);
  const amountB = new BN(balanceB.value.amount);

  if (amountA.isZero()) {
    throw new Error("Orca: base reserve (token A) is zero");
  }

  const SCALE = new BN(10).pow(new BN(18));
  const scaled = amountB.mul(SCALE).div(amountA);
  return Number(scaled.toString()) / 1e18;
}
