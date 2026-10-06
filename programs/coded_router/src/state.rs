use anchor_lang::prelude::*;

use crate::constants::*;
use crate::errors::CodedError;

/// Protocol-wide settings. One per deployment.
#[account]
#[derive(InitSpace)]
pub struct Global {
    /// Can edit the CPI allowlist, tip, fee allowance and pause flag,
    /// and can veto a bad holder epoch during its challenge window.
    pub admin: Pubkey,
    /// Key the indexer uses to post holder-distribution merkle roots.
    pub root_poster: Pubkey,
    /// Share of each inflow paid to whoever cranks `account_inflows`.
    pub crank_tip_bps: u16,
    /// Allowance for pump.fun trading fees when computing minimum outputs.
    pub fee_allowance_bps: u16,
    pub paused: bool,
    #[max_len(16)]
    pub allowed: Vec<AllowedCpi>,
    pub bump: u8,
    /// The protocol token every launch pays into (Pubkey::default() = off).
    pub protocol_mint: Pubkey,
    /// Router for the protocol token, seeds [router, mint, global].
    pub protocol_router: Pubkey,
    /// That router's vault: the address added to every new coin's pump.fun split.
    pub protocol_vault: Pubkey,
    /// Share of every new coin's creator fees owed to the protocol vault.
    pub protocol_fee_bps: u16,
}

/// A (program, instruction discriminator) pair the vault may sign for.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, InitSpace, PartialEq, Eq, Debug)]
pub struct AllowedCpi {
    pub program_id: Pubkey,
    pub discriminator: [u8; 8],
    /// CPI_KIND_SWAP or CPI_KIND_DEPOSIT
    pub kind: u8,
}

/// How the router's share of creator fees is split. Only the routes that
/// need on-chain logic live here; fixed-wallet routes (creator share,
/// custom wallets) are set directly in pump.fun's sharing_config.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, InitSpace, PartialEq, Eq, Debug)]
pub struct RouteConfig {
    pub holders_bps: u16,
    pub lp_bps: u16,
    pub burn_bps: u16,
    /// Minimum token balance (base units) to receive holder payouts.
    /// Enforced by the indexer when building the merkle tree; stored here
    /// so it's public and auditable.
    pub min_holder_balance: u64,
    pub max_slippage_bps: u16,
    pub lp_mode: u8,
    pub claim_interval_secs: i64,
    /// Exclude the creator wallet from holder payouts (indexer-enforced).
    pub exclude_creator: bool,
}

impl RouteConfig {
    pub fn validate(&self) -> Result<()> {
        let sum = self.holders_bps as u64 + self.lp_bps as u64 + self.burn_bps as u64;
        require!(sum == BPS, CodedError::BadRouteSum);
        require!(
            self.max_slippage_bps > 0 && self.max_slippage_bps <= MAX_SLIPPAGE_BPS,
            CodedError::SlippageTooHigh
        );
        require!(
            self.claim_interval_secs >= MIN_INTERVAL_SECS,
            CodedError::IntervalTooShort
        );
        require!(
            self.lp_mode == LP_MODE_BURN || self.lp_mode == LP_MODE_LOCK,
            CodedError::BadLpMode
        );
        Ok(())
    }
}

/// One router per (mint, authority). Its vault PDA is what gets listed as a
/// shareholder in pump.fun's sharing_config.
#[account]
#[derive(InitSpace)]
pub struct Router {
    pub mint: Pubkey,
    /// Can propose config changes (timelocked). Pubkey::default() = renounced.
    pub authority: Pubkey,
    /// Token program that owns `mint` (Token-2022 for create_v2 coins).
    pub token_program: Pubkey,

    pub config: RouteConfig,
    pub pending: Option<RouteConfig>,
    pub pending_eta: i64,

    // Accounting, all in lamports. Invariant:
    // holders_bucket + lp_bucket + burn_bucket + reserved_claims <= vault_total
    pub holders_bucket: u64,
    pub lp_bucket: u64,
    pub burn_bucket: u64,
    pub reserved_claims: u64,
    /// Tokens bought for liquidity, waiting to be deposited.
    pub lp_token_inventory: u64,

    pub lp_mint: Pubkey,
    pub lp_locked_amount: u64,
    pub lp_locked_until: i64,

    /// Reference price: quote per token, Q32 fixed point.
    pub ref_price_q32: u128,
    pub last_observe_ts: i64,
    pub last_inflow_ts: i64,
    pub next_epoch: u64,

    // Lifetime stats for the public page.
    pub total_inflow: u64,
    pub total_tokens_burned: u64,
    pub total_lp_quote_added: u64,
    pub total_paid_holders: u64,

    pub bump: u8,
    pub vault_bump: u8,

    /// Protocol share this coin owes, snapshotted at creation (0 = none).
    pub protocol_fee_bps: u16,
    /// Vault the protocol share must be paid to, snapshotted at creation.
    pub protocol_vault: Pubkey,
    /// Set once pump.fun's locked split is confirmed to include the share.
    pub protocol_verified: bool,
}

impl Router {
    pub fn accounted(&self) -> Result<u64> {
        self.holders_bucket
            .checked_add(self.lp_bucket)
            .and_then(|v| v.checked_add(self.burn_bucket))
            .and_then(|v| v.checked_add(self.reserved_claims))
            .ok_or(error!(CodedError::MathOverflow))
    }
}

/// A holder-distribution epoch. Paid by merkle proof.
#[account]
pub struct Epoch {
    pub router: Pubkey,
    pub index: u64,
    pub root: [u8; 32],
    pub total: u64,
    pub claimed: u64,
    pub num_nodes: u32,
    pub claimable_at: i64,
    pub expires_at: i64,
    pub vetoed: bool,
    pub poster: Pubkey,
    pub bump: u8,
    pub bitmap: Vec<u8>,
}

impl Epoch {
    pub const FIXED: usize = 8 + 32 + 8 + 32 + 8 + 8 + 4 + 8 + 8 + 1 + 32 + 1 + 4;

    pub fn bitmap_len(num_nodes: u32) -> usize {
        (num_nodes as usize + 7) / 8
    }

    pub fn space(num_nodes: u32) -> usize {
        Self::FIXED + Self::bitmap_len(num_nodes)
    }

    pub fn is_claimed(&self, i: u32) -> bool {
        self.bitmap[(i / 8) as usize] & (1 << (i % 8)) != 0
    }

    pub fn set_claimed(&mut self, i: u32) {
        self.bitmap[(i / 8) as usize] |= 1 << (i % 8);
    }
}
