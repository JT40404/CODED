//! Reading pump.fun's fee-sharing config to confirm the protocol share.

use anchor_lang::prelude::*;

use crate::constants::*;
use crate::errors::CodedError;

/// Total bps a locked sharing config pays to `vault`.
pub fn paid_to(data: &[u8], mint: &Pubkey, vault: &Pubkey) -> Result<u32> {
    require!(data.len() >= SC_SHAREHOLDERS_OFFSET, CodedError::BadSharingConfig);
    require!(data[..8] == SHARING_CONFIG_DISC, CodedError::BadSharingConfig);
    require!(
        data[SC_MINT_OFFSET..SC_MINT_OFFSET + 32] == mint.to_bytes(),
        CodedError::BadSharingConfig
    );
    // Must be locked, or the share could be removed after verifying.
    require!(data[SC_ADMIN_REVOKED_OFFSET] == 1, CodedError::SharingNotLocked);

    let n = u32::from_le_bytes(
        data[SC_SHAREHOLDERS_LEN_OFFSET..SC_SHAREHOLDERS_OFFSET]
            .try_into()
            .map_err(|_| CodedError::BadSharingConfig)?,
    ) as usize;
    require!(n <= 10, CodedError::BadSharingConfig);
    require!(
        data.len() >= SC_SHAREHOLDERS_OFFSET + n * SC_SHAREHOLDER_SIZE,
        CodedError::BadSharingConfig
    );
    let target = vault.to_bytes();
    let mut paid: u32 = 0;
    for i in 0..n {
        let o = SC_SHAREHOLDERS_OFFSET + i * SC_SHAREHOLDER_SIZE;
        if data[o..o + 32] == target {
            paid += u16::from_le_bytes([data[o + 32], data[o + 33]]) as u32;
        }
    }
    Ok(paid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(mint: &Pubkey, revoked: bool, holders: &[(Pubkey, u16)]) -> Vec<u8> {
        let mut d = SHARING_CONFIG_DISC.to_vec();
        d.extend([255u8, 1, 1]); // bump, version, status
        d.extend(mint.to_bytes());
        d.extend([9u8; 32]); // admin
        d.push(revoked as u8);
        d.extend((holders.len() as u32).to_le_bytes());
        for (k, b) in holders {
            d.extend(k.to_bytes());
            d.extend(b.to_le_bytes());
        }
        d
    }

    #[test]
    fn finds_protocol_share() {
        let mint = Pubkey::new_from_array([1; 32]);
        let vault = Pubkey::new_from_array([2; 32]);
        let other = Pubkey::new_from_array([3; 32]);
        let d = config(&mint, true, &[(other, 9_900), (vault, 100)]);
        assert_eq!(paid_to(&d, &mint, &vault).unwrap(), 100);
        let d = config(&mint, true, &[(other, 10_000)]);
        assert_eq!(paid_to(&d, &mint, &vault).unwrap(), 0);
        let d = config(&mint, false, &[(vault, 100), (other, 9_900)]);
        assert!(paid_to(&d, &mint, &vault).is_err()); // not locked
        let d = config(&other, true, &[(vault, 100), (other, 9_900)]);
        assert!(paid_to(&d, &mint, &vault).is_err()); // wrong mint
    }
}
