import { PublicKey } from "@solana/web3.js";
import {
  NATIVE_MINT,
  TOKEN_PROGRAM_ID,
  getAssociatedTokenAddressSync,
} from "@solana/spl-token";

/** Replace after `anchor keys sync`. */
export const CODED_PROGRAM_ID = new PublicKey(
  process.env.CODED_PROGRAM_ID ?? "Fg6PaFpoGXkYsidMpWTK6W2BeZ7FEfcYkg476zPFsLnS",
);

export const BPS = 10_000;
export const PURPOSE_BURN = 0;
export const PURPOSE_LP = 1;
export const VENUE_BONDING_CURVE = 0;
export const VENUE_AMM = 1;
export const LP_MODE_BURN = 0;
export const LP_MODE_LOCK = 1;
export const CPI_KIND_SWAP = 0;
export const CPI_KIND_DEPOSIT = 1;
export const VAULT_RENT_RESERVE = 890_880n;

const enc = (s: string) => Buffer.from(s);

export function globalPda(): PublicKey {
  return PublicKey.findProgramAddressSync([enc("global")], CODED_PROGRAM_ID)[0];
}

export function routerPda(mint: PublicKey, authority: PublicKey): PublicKey {
  return PublicKey.findProgramAddressSync(
    [enc("router"), mint.toBuffer(), authority.toBuffer()],
    CODED_PROGRAM_ID,
  )[0];
}

export function vaultPda(router: PublicKey): PublicKey {
  return PublicKey.findProgramAddressSync([enc("vault"), router.toBuffer()], CODED_PROGRAM_ID)[0];
}

export function epochPda(router: PublicKey, index: bigint | number): PublicKey {
  const i = Buffer.alloc(8);
  i.writeBigUInt64LE(BigInt(index));
  return PublicKey.findProgramAddressSync([enc("epoch"), router.toBuffer(), i], CODED_PROGRAM_ID)[0];
}

export function vaultWsolAta(vault: PublicKey): PublicKey {
  return getAssociatedTokenAddressSync(NATIVE_MINT, vault, true, TOKEN_PROGRAM_ID);
}
