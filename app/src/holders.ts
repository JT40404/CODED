/**
 * Holder snapshots and payout allocation for the holder route.
 */
import { Connection, PublicKey, SystemProgram } from "@solana/web3.js";
import type { Entry } from "./merkle";

export interface Holder {
  owner: PublicKey;
  balance: bigint;
}

/**
 * Every wallet holding `mint`, summed by owner. Only system-owned wallets are
 * kept: pools, curves, vaults and other program accounts are dropped, since
 * the on-chain claim only pays system accounts.
 */
export async function snapshotHolders(
  connection: Connection,
  mint: PublicKey,
  tokenProgram: PublicKey,
  opts: { minBalance: bigint; exclude: PublicKey[] },
): Promise<Holder[]> {
  const accounts = await connection.getProgramAccounts(tokenProgram, {
    filters: [{ memcmp: { offset: 0, bytes: mint.toBase58() } }],
    dataSlice: { offset: 32, length: 40 }, // owner (32) + amount (8)
  });

  const byOwner = new Map<string, bigint>();
  for (const { account } of accounts) {
    const d = account.data;
    if (d.length < 40) continue;
    const owner = new PublicKey(d.subarray(0, 32)).toBase58();
    const amount = d.readBigUInt64LE(32);
    if (amount === 0n) continue;
    byOwner.set(owner, (byOwner.get(owner) ?? 0n) + amount);
  }

  const excluded = new Set(opts.exclude.map((k) => k.toBase58()));
  const candidates = [...byOwner.entries()]
    .filter(([o, b]) => !excluded.has(o) && b >= opts.minBalance)
    .map(([o, b]) => ({ owner: new PublicKey(o), balance: b }));

  // Keep only wallets owned by the system program.
  const out: Holder[] = [];
  for (let i = 0; i < candidates.length; i += 100) {
    const chunk = candidates.slice(i, i + 100);
    const infos = await connection.getMultipleAccountsInfo(chunk.map((c) => c.owner));
    chunk.forEach((c, j) => {
      const info = infos[j];
      if (!info || info.owner.equals(SystemProgram.programId)) out.push(c);
    });
  }
  return out;
}

/**
 * Pro-rata split of `total` lamports. Payouts under `minPayout` are dropped
 * (a brand-new wallet can't receive less than the rent-exempt minimum).
 * Rounding dust stays in the bucket for the next epoch.
 */
export function allocate(holders: Holder[], total: bigint, minPayout: bigint): Entry[] {
  const supply = holders.reduce((a, h) => a + h.balance, 0n);
  if (supply === 0n) return [];
  let entries = holders.map((h) => ({ claimant: h.owner, amount: (total * h.balance) / supply }));
  entries = entries.filter((e) => e.amount >= minPayout);
  // Re-spread what the dropped wallets would have had.
  const kept = holders.filter((h) => entries.some((e) => e.claimant.equals(h.owner)));
  const keptSupply = kept.reduce((a, h) => a + h.balance, 0n);
  if (keptSupply === 0n) return [];
  return kept
    .map((h) => ({ claimant: h.owner, amount: (total * h.balance) / keptSupply }))
    .sort((a, b) => (a.claimant.toBase58() < b.claimant.toBase58() ? -1 : 1));
}
