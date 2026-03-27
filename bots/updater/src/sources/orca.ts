import { Connection, PublicKey } from "@solana/web3.js";
import BN from "bn.js";

const ORCA_WHIRLPOOL_PROGRAM_ID = new PublicKey(
  "whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc"
);

// Orca Whirlpool account layout offsets (after 8-byte Anchor discriminator).
const WHIRLPOOL_MIN_SIZE = 229;
const MINT_A_OFFSET = 101;
const MINT_B_OFFSET = 133;
const VAULT_A_OFFSET = 165;
const VAULT_B_OFFSET = 197;

function computePrice(numerator: BN, denominator: BN): number {
  if (denominator.isZero()) {
    throw new Error("Orca: denominator reserve is zero");
  }
  const SCALE = new BN(10).pow(new BN(18));
  const scaled = numerator.mul(SCALE).div(denominator);
  return Number(scaled.toString()) / 1e18;
}

/**
 * Fetches the spot price from an Orca Whirlpool.
 * Validates that pool mints match the oracle's base/quote mints.
 * If the pool token ordering is reversed, the price is inverted.
 */
export async function fetchPrice(
  connection: Connection,
  poolAddress: PublicKey,
  baseMint: PublicKey,
  quoteMint: PublicKey
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
  const mintA = new PublicKey(data.subarray(MINT_A_OFFSET, MINT_A_OFFSET + 32));
  const mintB = new PublicKey(data.subarray(MINT_B_OFFSET, MINT_B_OFFSET + 32));

  const forward = mintA.equals(baseMint) && mintB.equals(quoteMint);
  const reversed = mintA.equals(quoteMint) && mintB.equals(baseMint);

  if (!forward && !reversed) {
    throw new Error(
      `Orca: pool mints (${mintA.toBase58()}, ${mintB.toBase58()}) ` +
        `do not match oracle mints (${baseMint.toBase58()}, ${quoteMint.toBase58()})`
    );
  }

  const vaultA = new PublicKey(data.subarray(VAULT_A_OFFSET, VAULT_A_OFFSET + 32));
  const vaultB = new PublicKey(data.subarray(VAULT_B_OFFSET, VAULT_B_OFFSET + 32));

  const [balanceA, balanceB] = await Promise.all([
    connection.getTokenAccountBalance(vaultA),
    connection.getTokenAccountBalance(vaultB),
  ]);

  const amountA = new BN(balanceA.value.amount);
  const amountB = new BN(balanceB.value.amount);

  // price = B / A in pool terms
  const rawPrice = computePrice(amountB, amountA);
  return reversed ? 1 / rawPrice : rawPrice;
}
