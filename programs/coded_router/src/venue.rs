//! Reading prices from pump.fun's bonding curve and PumpSwap pools without
//! trusting the cranker. Every account is re-derived from its seeds.

use anchor_lang::prelude::*;
use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use anchor_spl::token::ID as SPL_TOKEN_ID;

use crate::constants::*;
use crate::errors::CodedError;

#[derive(Clone, Copy, Debug)]
pub struct Reserves {
    pub token: u64,
    pub quote: u64,
}

impl Reserves {
    /// Quote per token in Q32 fixed point.
    pub fn price_q32(&self) -> Result<u128> {
        require!(self.token > 0, CodedError::BadVenue);
        Ok(((self.quote as u128) << 32) / self.token as u128)
    }

    /// Constant-product output for `quote_in`, before fees.
    pub fn tokens_out(&self, quote_in: u64) -> Result<u64> {
        let num = (self.token as u128)
            .checked_mul(quote_in as u128)
            .ok_or(CodedError::MathOverflow)?;
        let den = (self.quote as u128)
            .checked_add(quote_in as u128)
            .ok_or(CodedError::MathOverflow)?;
        Ok((num / den) as u64)
    }
}

fn read_u64(data: &[u8], off: usize) -> Result<u64> {
    let bytes: [u8; 8] = data
        .get(off..off + 8)
        .ok_or(CodedError::BadVenue)?
        .try_into()
        .map_err(|_| CodedError::BadVenue)?;
    Ok(u64::from_le_bytes(bytes))
}

pub fn token_amount(acc: &AccountInfo) -> Result<u64> {
    if acc.data_is_empty() {
        return Ok(0);
    }
    read_u64(&acc.try_borrow_data()?, TOKEN_ACCOUNT_AMOUNT_OFFSET)
}

pub fn mint_supply(acc: &AccountInfo) -> Result<u64> {
    read_u64(&acc.try_borrow_data()?, MINT_SUPPLY_OFFSET)
}

/// Bonding curve venue. `a` = bonding_curve PDA.
pub fn bonding_curve_reserves(a: &AccountInfo, mint: &Pubkey) -> Result<Reserves> {
    let (expected, _) =
        Pubkey::find_program_address(&[PUMP_BONDING_CURVE_SEED, mint.as_ref()], &PUMP_PROGRAM_ID);
    require_keys_eq!(*a.key, expected, CodedError::BadVenue);
    require_keys_eq!(*a.owner, PUMP_PROGRAM_ID, CodedError::BadVenue);
    let data = a.try_borrow_data()?;
    require!(data.len() > BC_COMPLETE_OFFSET, CodedError::BadVenue);
    require!(data[BC_COMPLETE_OFFSET] == 0, CodedError::CurveComplete);
    Ok(Reserves {
        token: read_u64(&data, BC_VIRTUAL_TOKEN_RESERVES_OFFSET)?,
        quote: read_u64(&data, BC_VIRTUAL_QUOTE_RESERVES_OFFSET)?,
    })
}

/// Canonical PumpSwap pool for a pump.fun coin.
pub fn canonical_pool(mint: &Pubkey) -> Pubkey {
    let (pool_authority, _) = Pubkey::find_program_address(
        &[PUMP_POOL_AUTHORITY_SEED, mint.as_ref()],
        &PUMP_PROGRAM_ID,
    );
    Pubkey::find_program_address(
        &[
            PUMP_AMM_POOL_SEED,
            &CANONICAL_POOL_INDEX.to_le_bytes(),
            pool_authority.as_ref(),
            mint.as_ref(),
            WSOL_MINT.as_ref(),
        ],
        &PUMP_AMM_PROGRAM_ID,
    )
    .0
}

/// AMM venue. `a` = pool, `b` = pool base token account, `c` = pool quote
/// (WSOL) token account. Note: PumpSwap prices against
/// quote balance + Pool::virtual_quote_reserves, which is 0 on all pools
/// today. If pump starts using it, read it here.
pub fn amm_reserves(
    a: &AccountInfo,
    b: &AccountInfo,
    c: &AccountInfo,
    mint: &Pubkey,
    base_token_program: &Pubkey,
) -> Result<Reserves> {
    require_keys_eq!(*a.key, canonical_pool(mint), CodedError::BadVenue);
    require_keys_eq!(*a.owner, PUMP_AMM_PROGRAM_ID, CodedError::BadVenue);
    let base_ata = get_associated_token_address_with_program_id(a.key, mint, base_token_program);
    let quote_ata = get_associated_token_address_with_program_id(a.key, &WSOL_MINT, &SPL_TOKEN_ID);
    require_keys_eq!(*b.key, base_ata, CodedError::BadVenue);
    require_keys_eq!(*c.key, quote_ata, CodedError::BadVenue);
    Ok(Reserves {
        token: token_amount(b)?,
        quote: token_amount(c)?,
    })
}

pub fn read_reserves(
    venue: u8,
    a: &AccountInfo,
    b: &AccountInfo,
    c: &AccountInfo,
    mint: &Pubkey,
    base_token_program: &Pubkey,
) -> Result<Reserves> {
    match venue {
        VENUE_BONDING_CURVE => bonding_curve_reserves(a, mint),
        VENUE_AMM => amm_reserves(a, b, c, mint, base_token_program),
        _ => err!(CodedError::BadVenue),
    }
}

/// Spot must be within `max_bps` of the reference price.
pub fn check_deviation(reference: u128, spot: u128, max_bps: u16) -> Result<()> {
    require!(reference > 0, CodedError::NoReferencePrice);
    let diff = if spot > reference { spot - reference } else { reference - spot };
    let limit = reference
        .checked_mul(max_bps as u128)
        .ok_or(CodedError::MathOverflow)?
        / BPS as u128;
    require!(diff <= limit, CodedError::PriceDeviation);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_math() {
        let r = Reserves { token: 1_000_000_000, quote: 30_000_000_000 };
        // 1% of quote in → just under 1% of tokens out
        let out = r.tokens_out(300_000_000).unwrap();
        assert_eq!(out, 9_900_990);
        assert!(check_deviation(100, 104, 500).is_ok());
        assert!(check_deviation(100, 106, 500).is_err());
        assert!(check_deviation(0, 1, 500).is_err());
    }
}
