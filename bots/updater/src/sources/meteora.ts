import { Connection, PublicKey } from "@solana/web3.js";
import BN from "bn.js";

const METEORA_DLMM_PROGRAM_ID = new PublicKey(
  "LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo"
);

// Meteora DLMM LbPair account layout offsets (after 8-byte Anchor discriminator).
// StaticParameters (30 bytes) + VariableParameters (32 bytes) + misc fields,
// then: token_x_mint at 88, token_y_mint at 120, reserve_x at 152, reserve_y at 184.
const LB_PAIR_MIN_SIZE = 216;
const RESERVE_X_OFFSET = 152;
const RESERVE_Y_OFFSET = 184;

/**
 * Fetches the spot price from a Meteora DLMM pool.
 * Reads reserve token balances and returns price as quote / base (floating point).
 *
 * Assumes token X is the base token and token Y is the quote token.
 */
export async function fetchPrice(
  connection: Connection,
  poolAddress: PublicKey
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
  const reserveX = new PublicKey(
    data.subarray(RESERVE_X_OFFSET, RESERVE_X_OFFSET + 32)
  );
  const reserveY = new PublicKey(
    data.subarray(RESERVE_Y_OFFSET, RESERVE_Y_OFFSET + 32)
  );

  const [balanceX, balanceY] = await Promise.all([
    connection.getTokenAccountBalance(reserveX),
    connection.getTokenAccountBalance(reserveY),
  ]);

  const amountX = new BN(balanceX.value.amount);
  const amountY = new BN(balanceY.value.amount);

  if (amountX.isZero()) {
    throw new Error("Meteora: base reserve (token X) is zero");
  }

  const SCALE = new BN(10).pow(new BN(18));
  const scaled = amountY.mul(SCALE).div(amountX);
  return Number(scaled.toString()) / 1e18;
}
