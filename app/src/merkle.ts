import { keccak_256 } from "@noble/hashes/sha3";
import { PublicKey } from "@solana/web3.js";

/** Must match programs/coded_router/src/merkle.rs exactly. */
export function leafHash(epoch: PublicKey, index: number, claimant: PublicKey, amount: bigint): Buffer {
  const idx = Buffer.alloc(4);
  idx.writeUInt32LE(index);
  const amt = Buffer.alloc(8);
  amt.writeBigUInt64LE(amount);
  return Buffer.from(
    keccak_256(Buffer.concat([Buffer.from([0]), epoch.toBuffer(), idx, claimant.toBuffer(), amt])),
  );
}

function nodeHash(a: Buffer, b: Buffer): Buffer {
  const [x, y] = Buffer.compare(a, b) <= 0 ? [a, b] : [b, a];
  return Buffer.from(keccak_256(Buffer.concat([Buffer.from([1]), x, y])));
}

export interface Entry {
  claimant: PublicKey;
  amount: bigint;
}

export interface Tree {
  root: Buffer;
  leaves: Buffer[];
  levels: Buffer[][];
}

/** Pairs adjacent nodes; an odd node is promoted unchanged. */
export function buildTree(epoch: PublicKey, entries: Entry[]): Tree {
  if (entries.length === 0) throw new Error("empty tree");
  const leaves = entries.map((e, i) => leafHash(epoch, i, e.claimant, e.amount));
  const levels: Buffer[][] = [leaves];
  while (levels[levels.length - 1].length > 1) {
    const cur = levels[levels.length - 1];
    const next: Buffer[] = [];
    for (let i = 0; i < cur.length; i += 2) {
      next.push(i + 1 < cur.length ? nodeHash(cur[i], cur[i + 1]) : cur[i]);
    }
    levels.push(next);
  }
  return { root: levels[levels.length - 1][0], leaves, levels };
}

export function proofFor(tree: Tree, index: number): Buffer[] {
  const proof: Buffer[] = [];
  let i = index;
  for (let l = 0; l < tree.levels.length - 1; l++) {
    const level = tree.levels[l];
    const sib = i ^ 1;
    if (sib < level.length) proof.push(level[sib]);
    i = Math.floor(i / 2);
  }
  return proof;
}

export function verify(proof: Buffer[], root: Buffer, leaf: Buffer): boolean {
  let h = leaf;
  for (const p of proof) h = nodeHash(h, p);
  return h.equals(root);
}
