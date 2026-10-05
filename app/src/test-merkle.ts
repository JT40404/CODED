// Cross-checks the TS tree against the vector printed by `cargo test`.
import { PublicKey } from "@solana/web3.js";
import { buildTree, proofFor, verify } from "./merkle";

const EXPECTED_ROOT = "3ec2efbbd8fc9bbb1750836e11c7907065e42ad14a6328791503fa69d6d40728";
const epoch = new PublicKey(Buffer.alloc(32, 7));
const entries = [1, 2, 3].map((i) => ({
  claimant: new PublicKey(Buffer.alloc(32, i)),
  amount: BigInt(i * 1_000_000),
}));
const tree = buildTree(epoch, entries);
const root = tree.root.toString("hex");
if (root !== EXPECTED_ROOT) throw new Error(`root mismatch: ${root}`);
entries.forEach((_, i) => {
  if (!verify(proofFor(tree, i), tree.root, tree.leaves[i])) throw new Error(`proof ${i} failed`);
});
console.log("merkle ok", root);
