use anchor_lang::prelude::*;
use anchor_lang::solana_program::keccak::hashv;

/// Leaf = keccak(0x00 || epoch || index_le || claimant || amount_le).
/// The 0x00 / 0x01 prefixes separate leaves from inner nodes so a node
/// can never be passed off as a leaf.
pub fn leaf(epoch: &Pubkey, index: u32, claimant: &Pubkey, amount: u64) -> [u8; 32] {
    hashv(&[
        &[0u8],
        epoch.as_ref(),
        &index.to_le_bytes(),
        claimant.as_ref(),
        &amount.to_le_bytes(),
    ])
    .0
}

/// Sorted-pair inner nodes: keccak(0x01 || min(a,b) || max(a,b)).
pub fn verify(proof: &[[u8; 32]], root: &[u8; 32], leaf: [u8; 32]) -> bool {
    let mut h = leaf;
    for p in proof {
        h = if h <= *p {
            hashv(&[&[1u8], &h, p]).0
        } else {
            hashv(&[&[1u8], p, &h]).0
        };
    }
    &h == root
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(a: [u8; 32], b: [u8; 32]) -> [u8; 32] {
        if a <= b { hashv(&[&[1u8], &a, &b]).0 } else { hashv(&[&[1u8], &b, &a]).0 }
    }

    #[test]
    fn three_leaf_tree() {
        let epoch = Pubkey::new_from_array([7u8; 32]);
        let ws: Vec<Pubkey> = (1..=3u8).map(|i| Pubkey::new_from_array([i; 32])).collect();
        let amts = [1_000_000u64, 2_000_000, 3_000_000];
        let l: Vec<[u8; 32]> = (0..3).map(|i| leaf(&epoch, i as u32, &ws[i], amts[i])).collect();
        // odd leaf is promoted unchanged
        let n01 = node(l[0], l[1]);
        let root = node(n01, l[2]);
        assert!(verify(&[l[1], l[2]], &root, l[0]));
        assert!(verify(&[l[0], l[2]], &root, l[1]));
        assert!(verify(&[n01], &root, l[2]));
        // wrong amount fails
        assert!(!verify(&[l[1], l[2]], &root, leaf(&epoch, 0, &ws[0], amts[0] + 1)));
        // print vector for the TS cross-check
        println!("ROOT {}", root.iter().map(|b| format!("{:02x}", b)).collect::<String>());
    }
}
