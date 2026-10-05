//! Guarded CPI: the vault PDA signs exactly one allowlisted instruction on
//! pump.fun's programs. The caller then checks balance deltas, so even a
//! malformed payload can't take more than the approved amount.

use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::solana_program::program::invoke_signed;

use crate::constants::*;
use crate::errors::CodedError;
use crate::state::Global;

/// `rem[0]` is the target program. `rem[1..]` are the instruction's accounts
/// in order. The vault is marked as signer wherever it appears.
pub fn guarded_invoke<'info>(
    global: &Global,
    kind: u8,
    rem: &[AccountInfo<'info>],
    payload: &[u8],
    vault: &Pubkey,
    signer_seeds: &[&[&[u8]]],
) -> Result<()> {
    require!(!rem.is_empty(), CodedError::CpiNotAllowed);
    require!(payload.len() >= 8, CodedError::BadPayload);
    let program = &rem[0];
    require!(
        *program.key == PUMP_PROGRAM_ID || *program.key == PUMP_AMM_PROGRAM_ID,
        CodedError::CpiNotAllowed
    );
    let disc: [u8; 8] = payload[..8].try_into().map_err(|_| CodedError::BadPayload)?;
    require!(
        global
            .allowed
            .iter()
            .any(|a| a.program_id == *program.key && a.discriminator == disc && a.kind == kind),
        CodedError::CpiNotAllowed
    );

    let metas: Vec<AccountMeta> = rem[1..]
        .iter()
        .map(|a| AccountMeta {
            pubkey: *a.key,
            is_signer: a.is_signer || a.key == vault,
            is_writable: a.is_writable,
        })
        .collect();

    let ix = Instruction {
        program_id: *program.key,
        accounts: metas,
        data: payload.to_vec(),
    };
    invoke_signed(&ix, rem, signer_seeds)?;
    Ok(())
}
