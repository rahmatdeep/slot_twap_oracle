import { Connection, PublicKey } from "@solana/web3.js";
import BN from "bn.js";

const METEORA_DLMM_PROGRAM_ID = new PublicKey(
  "LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo"
);

// Meteora DLMM LbPair account layout offsets (after 8-byte Anchor discriminator).
// token_x_mint at 88, token_y_mint at 120, reserve_x at 152, reserve_y at 184.
const LB_PAIR_MIN_SIZE = 216;
const MINT_X_OFFSET = 88;
const MINT_Y_OFFSET = 120;
const RESERVE_X_OFFSET = 152;
const RESERVE_Y_OFFSET = 184;

function computePrice(numerator: BN, denominator: BN): number {
  if (denominator.isZero()) {
    throw new Error("Meteora: denominator reserve is zero");
  }
  const SCALE = new BN(10).pow(new BN(18));
  const scaled = numerator.mul(SCALE).div(denominator);
  return Number(scaled.toString()) / 1e18;
}

/**
 * Fetches the spot price from a Meteora DLMM pool.
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
    throw new Error(`Meteora LB pair not found: ${poolAddress.toBase58()}`);

  if (!poolAccount.owner.equals(METEORA_DLMM_PROGRAM_ID)) {
    throw new Error(
      `Meteora LB pair ${poolAddress.toBase58()} not owned by DLMM program`
    );
  }

  if (poolAccount.data.length < LB_PAIR_MIN_SIZE) {
    throw new Error(
      `Meteora LB pair data too small: expected >= ${LB_PAIR_MIN_SIZE}, got ${poolAccount.data.length}`
    );
  }

  const data = poolAccount.data;
  const mintX = new PublicKey(data.subarray(MINT_X_OFFSET, MINT_X_OFFSET + 32));
  const mintY = new PublicKey(data.subarray(MINT_Y_OFFSET, MINT_Y_OFFSET + 32));

  const forward = mintX.equals(baseMint) && mintY.equals(quoteMint);
  const reversed = mintX.equals(quoteMint) && mintY.equals(baseMint);

  if (!forward && !reversed) {
    throw new Error(
      `Meteora: pool mints (${mintX.toBase58()}, ${mintY.toBase58()}) ` +
        `do not match oracle mints (${baseMint.toBase58()}, ${quoteMint.toBase58()})`
    );
  }

  const reserveX = new PublicKey(data.subarray(RESERVE_X_OFFSET, RESERVE_X_OFFSET + 32));
  const reserveY = new PublicKey(data.subarray(RESERVE_Y_OFFSET, RESERVE_Y_OFFSET + 32));

  const [balanceX, balanceY] = await Promise.all([
    connection.getTokenAccountBalance(reserveX),
    connection.getTokenAccountBalance(reserveY),
  ]);

  const amountX = new BN(balanceX.value.amount);
  const amountY = new BN(balanceY.value.amount);

  // price = Y / X in pool terms
  const rawPrice = computePrice(amountY, amountX);
  return reversed ? 1 / rawPrice : rawPrice;
}
