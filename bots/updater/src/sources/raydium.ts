import { Connection, PublicKey } from "@solana/web3.js";
import BN from "bn.js";

const RAYDIUM_AMM_PROGRAM_ID = new PublicKey(
  "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8"
);

// Raydium AMM v4 account layout offsets.
const AMM_ACCOUNT_MIN_SIZE = 752;
const BASE_MINT_OFFSET = 400;
const QUOTE_MINT_OFFSET = 432;
const BASE_VAULT_OFFSET = 336;
const QUOTE_VAULT_OFFSET = 368;

function computePrice(numerator: BN, denominator: BN): number {
  if (denominator.isZero()) {
    throw new Error("Raydium: denominator reserve is zero");
  }
  const SCALE = new BN(10).pow(new BN(18));
  const scaled = numerator.mul(SCALE).div(denominator);
  return Number(scaled.toString()) / 1e18;
}

/**
 * Fetches the spot price from a Raydium AMM v4 pool.
 * Validates that pool mints match the oracle's base/quote mints.
 * If the pool token ordering is reversed, the price is inverted.
 */
export async function fetchPrice(
  connection: Connection,
  poolAddress: PublicKey,
  baseMint: PublicKey,
  quoteMint: PublicKey
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
  const poolBaseMint = new PublicKey(data.subarray(BASE_MINT_OFFSET, BASE_MINT_OFFSET + 32));
  const poolQuoteMint = new PublicKey(data.subarray(QUOTE_MINT_OFFSET, QUOTE_MINT_OFFSET + 32));

  const forward = poolBaseMint.equals(baseMint) && poolQuoteMint.equals(quoteMint);
  const reversed = poolBaseMint.equals(quoteMint) && poolQuoteMint.equals(baseMint);

  if (!forward && !reversed) {
    throw new Error(
      `Raydium: pool mints (${poolBaseMint.toBase58()}, ${poolQuoteMint.toBase58()}) ` +
        `do not match oracle mints (${baseMint.toBase58()}, ${quoteMint.toBase58()})`
    );
  }

  const baseVault = new PublicKey(data.subarray(BASE_VAULT_OFFSET, BASE_VAULT_OFFSET + 32));
  const quoteVault = new PublicKey(data.subarray(QUOTE_VAULT_OFFSET, QUOTE_VAULT_OFFSET + 32));

  const [baseBalance, quoteBalance] = await Promise.all([
    connection.getTokenAccountBalance(baseVault),
    connection.getTokenAccountBalance(quoteVault),
  ]);

  const baseAmount = new BN(baseBalance.value.amount);
  const quoteAmount = new BN(quoteBalance.value.amount);

  // price = quote / base in pool terms
  const rawPrice = computePrice(quoteAmount, baseAmount);
  return reversed ? 1 / rawPrice : rawPrice;
}
