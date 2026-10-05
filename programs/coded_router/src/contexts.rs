use anchor_lang::prelude::*;
use anchor_spl::associated_token::AssociatedToken;
use anchor_spl::token::Token;
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};

use crate::constants::*;
use crate::errors::CodedError;
use crate::state::*;

// ----------------------------------------------------------------- global

#[derive(Accounts)]
pub struct InitGlobal<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,
    #[account(init, payer = admin, space = 8 + Global::INIT_SPACE, seeds = [GLOBAL_SEED], bump)]
    pub global: Account<'info, Global>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct AdminOnly<'info> {
    pub admin: Signer<'info>,
    #[account(mut, seeds = [GLOBAL_SEED], bump = global.bump, has_one = admin @ CodedError::Unauthorized)]
    pub global: Account<'info, Global>,
}

// ----------------------------------------------------------------- router

#[derive(Accounts)]
pub struct InitializeRouter<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,
    #[account(seeds = [GLOBAL_SEED], bump = global.bump)]
    pub global: Box<Account<'info, Global>>,
    pub mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(
        init,
        payer = authority,
        space = 8 + Router::INIT_SPACE,
        seeds = [ROUTER_SEED, mint.key().as_ref(), authority.key().as_ref()],
        bump
    )]
    pub router: Box<Account<'info, Router>>,
    #[account(mut, seeds = [VAULT_SEED, router.key().as_ref()], bump)]
    pub vault: SystemAccount<'info>,
    /// CHECK: verified in venue::read_reserves
    pub venue_a: UncheckedAccount<'info>,
    /// CHECK: verified in venue::read_reserves
    pub venue_b: UncheckedAccount<'info>,
    /// CHECK: verified in venue::read_reserves
    pub venue_c: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct AuthorityOnly<'info> {
    pub authority: Signer<'info>,
    #[account(mut, has_one = authority @ CodedError::Unauthorized)]
    pub router: Account<'info, Router>,
}

#[derive(Accounts)]
pub struct ApplyConfig<'info> {
    #[account(mut)]
    pub router: Account<'info, Router>,
}

#[derive(Accounts)]
pub struct ObservePrice<'info> {
    #[account(mut)]
    pub router: Account<'info, Router>,
    /// CHECK: verified in venue::read_reserves
    pub venue_a: UncheckedAccount<'info>,
    /// CHECK: verified in venue::read_reserves
    pub venue_b: UncheckedAccount<'info>,
    /// CHECK: verified in venue::read_reserves
    pub venue_c: UncheckedAccount<'info>,
}

// ----------------------------------------------------------------- inflows

#[derive(Accounts)]
pub struct AccountInflows<'info> {
    #[account(mut)]
    pub cranker: Signer<'info>,
    #[account(seeds = [GLOBAL_SEED], bump = global.bump)]
    pub global: Box<Account<'info, Global>>,
    #[account(mut)]
    pub router: Box<Account<'info, Router>>,
    #[account(mut, seeds = [VAULT_SEED, router.key().as_ref()], bump = router.vault_bump)]
    pub vault: SystemAccount<'info>,
    /// CHECK: must equal ATA(vault, WSOL); checked in handler. May be uninitialized.
    pub vault_wsol: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

// ----------------------------------------------------------------- buy

#[derive(Accounts)]
pub struct ExecuteBuy<'info> {
    #[account(mut)]
    pub cranker: Signer<'info>,
    #[account(seeds = [GLOBAL_SEED], bump = global.bump)]
    pub global: Box<Account<'info, Global>>,
    #[account(mut)]
    pub router: Box<Account<'info, Router>>,
    #[account(mut, seeds = [VAULT_SEED, router.key().as_ref()], bump = router.vault_bump)]
    pub vault: SystemAccount<'info>,
    #[account(mut, address = router.mint)]
    pub mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(
        init_if_needed,
        payer = cranker,
        associated_token::mint = mint,
        associated_token::authority = vault,
        associated_token::token_program = token_program
    )]
    pub vault_token_account: Box<InterfaceAccount<'info, TokenAccount>>,
    /// CHECK: must equal ATA(vault, WSOL); checked in handler.
    #[account(mut)]
    pub vault_wsol: UncheckedAccount<'info>,
    /// CHECK: verified in venue::read_reserves
    pub venue_a: UncheckedAccount<'info>,
    /// CHECK: verified in venue::read_reserves
    pub venue_b: UncheckedAccount<'info>,
    /// CHECK: verified in venue::read_reserves
    pub venue_c: UncheckedAccount<'info>,
    #[account(address = router.token_program)]
    pub token_program: Interface<'info, TokenInterface>,
    pub spl_token_program: Program<'info, Token>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

// ----------------------------------------------------------------- deposit

#[derive(Accounts)]
pub struct ExecuteDeposit<'info> {
    #[account(mut)]
    pub cranker: Signer<'info>,
    #[account(seeds = [GLOBAL_SEED], bump = global.bump)]
    pub global: Box<Account<'info, Global>>,
    #[account(mut)]
    pub router: Box<Account<'info, Router>>,
    #[account(mut, seeds = [VAULT_SEED, router.key().as_ref()], bump = router.vault_bump)]
    pub vault: SystemAccount<'info>,
    #[account(address = router.mint)]
    pub mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = vault,
        associated_token::token_program = token_program
    )]
    pub vault_token_account: Box<InterfaceAccount<'info, TokenAccount>>,
    /// CHECK: must equal ATA(vault, WSOL); checked in handler.
    #[account(mut)]
    pub vault_wsol: UncheckedAccount<'info>,
    /// CHECK: verified in venue::amm_reserves
    pub pool: UncheckedAccount<'info>,
    /// CHECK: verified in venue::amm_reserves
    pub pool_base: UncheckedAccount<'info>,
    /// CHECK: verified in venue::amm_reserves
    pub pool_quote: UncheckedAccount<'info>,
    #[account(mut)]
    pub lp_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(
        init_if_needed,
        payer = cranker,
        associated_token::mint = lp_mint,
        associated_token::authority = vault,
        associated_token::token_program = lp_token_program
    )]
    pub vault_lp_account: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(address = router.token_program)]
    pub token_program: Interface<'info, TokenInterface>,
    pub lp_token_program: Interface<'info, TokenInterface>,
    pub spl_token_program: Program<'info, Token>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

// ----------------------------------------------------------------- lp lock

#[derive(Accounts)]
pub struct ReleaseLockedLp<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,
    #[account(mut, has_one = authority @ CodedError::Unauthorized, has_one = lp_mint @ CodedError::LpMintMismatch)]
    pub router: Box<Account<'info, Router>>,
    #[account(seeds = [VAULT_SEED, router.key().as_ref()], bump = router.vault_bump)]
    pub vault: SystemAccount<'info>,
    pub lp_mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(
        mut,
        associated_token::mint = lp_mint,
        associated_token::authority = vault,
        associated_token::token_program = lp_token_program
    )]
    pub vault_lp_account: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(
        init_if_needed,
        payer = authority,
        associated_token::mint = lp_mint,
        associated_token::authority = authority,
        associated_token::token_program = lp_token_program
    )]
    pub authority_lp_account: Box<InterfaceAccount<'info, TokenAccount>>,
    pub lp_token_program: Interface<'info, TokenInterface>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct UnwrapWsol<'info> {
    pub router: Account<'info, Router>,
    #[account(mut, seeds = [VAULT_SEED, router.key().as_ref()], bump = router.vault_bump)]
    pub vault: SystemAccount<'info>,
    /// CHECK: must equal ATA(vault, WSOL); checked in handler.
    #[account(mut)]
    pub vault_wsol: UncheckedAccount<'info>,
    pub spl_token_program: Program<'info, Token>,
}

// ----------------------------------------------------------------- epochs

#[derive(Accounts)]
#[instruction(index: u64, root: [u8; 32], total: u64, num_nodes: u32)]
pub struct PostEpoch<'info> {
    #[account(mut)]
    pub poster: Signer<'info>,
    #[account(seeds = [GLOBAL_SEED], bump = global.bump, constraint = global.root_poster == poster.key() @ CodedError::Unauthorized)]
    pub global: Box<Account<'info, Global>>,
    #[account(mut)]
    pub router: Box<Account<'info, Router>>,
    #[account(
        init,
        payer = poster,
        space = Epoch::space(num_nodes),
        seeds = [EPOCH_SEED, router.key().as_ref(), &index.to_le_bytes()],
        bump
    )]
    pub epoch: Box<Account<'info, Epoch>>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct VetoEpoch<'info> {
    pub signer: Signer<'info>,
    #[account(seeds = [GLOBAL_SEED], bump = global.bump)]
    pub global: Box<Account<'info, Global>>,
    #[account(mut)]
    pub router: Box<Account<'info, Router>>,
    #[account(
        mut,
        has_one = router,
        has_one = poster,
        close = poster,
        seeds = [EPOCH_SEED, router.key().as_ref(), &epoch.index.to_le_bytes()],
        bump = epoch.bump
    )]
    pub epoch: Box<Account<'info, Epoch>>,
    /// CHECK: receives the epoch account's rent; matched by has_one.
    #[account(mut)]
    pub poster: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct Claim<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(mut)]
    pub router: Box<Account<'info, Router>>,
    #[account(mut, seeds = [VAULT_SEED, router.key().as_ref()], bump = router.vault_bump)]
    pub vault: SystemAccount<'info>,
    #[account(
        mut,
        has_one = router,
        seeds = [EPOCH_SEED, router.key().as_ref(), &epoch.index.to_le_bytes()],
        bump = epoch.bump
    )]
    pub epoch: Box<Account<'info, Epoch>>,
    /// CHECK: any system-owned wallet; bound into the merkle leaf.
    #[account(mut)]
    pub claimant: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct CloseEpoch<'info> {
    #[account(mut)]
    pub router: Box<Account<'info, Router>>,
    #[account(
        mut,
        has_one = router,
        has_one = poster,
        close = poster,
        seeds = [EPOCH_SEED, router.key().as_ref(), &epoch.index.to_le_bytes()],
        bump = epoch.bump
    )]
    pub epoch: Box<Account<'info, Epoch>>,
    /// CHECK: receives rent; matched by has_one.
    #[account(mut)]
    pub poster: UncheckedAccount<'info>,
}
