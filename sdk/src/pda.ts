import { PublicKey } from "@solana/web3.js";

export const PROGRAM_ID = new PublicKey(
  "7LKj9Yk62ddRjtTHvvV6fmquD9h7XbcvKKa7yGtocdsT"
);

export function findOraclePda(
  baseMint: PublicKey,
  quoteMint: PublicKey,
  programId: PublicKey = PROGRAM_ID
): [PublicKey, number] {
  return PublicKey.findProgramAddressSync(
    [Buffer.from("oracle"), baseMint.toBuffer(), quoteMint.toBuffer()],
    programId
  );
}

export function findObservationBufferPda(
  oracle: PublicKey,
  programId: PublicKey = PROGRAM_ID
): [PublicKey, number] {
  return PublicKey.findProgramAddressSync(
    [Buffer.from("observation"), oracle.toBuffer()],
    programId
  );
}
