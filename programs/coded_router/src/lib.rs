//! CODED router
//!
//! Receives a coin's creator fees as a shareholder in pump.fun's
//! sharing_config, then routes them three ways:
//!   * holders  – merkle-distributed SOL to token holders
//!   * lp       – buys the token and adds liquidity on PumpSwap
//!   * burn     – buys the token and burns it
//!
//! Fixed-wallet routes (creator share, custom wallets) are not handled here.
//! They are set directly as shareholders in pump.fun's sharing_config, which
//! is made permanent at launch.
//!
//! Instructions on pump.fun's programs are built off-chain with pump's own
//! SDKs and passed in as an opaque payload. The program never trusts that
//! payload: it checks the target against an allowlist, re-derives every
//! venue account, bounds trade size by pool depth, checks spot against a
//! slow reference price, and verifies balance changes after the CPI.

use anchor_lang::prelude::*;
use anchor_lang::system_program::{self, Transfer as SysTransfer};
use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use anchor_spl::token::{self as spl, CloseAccount, SyncNative, ID as SPL_TOKEN_ID};
use anchor_spl::token_interface::{self as ti, Burn, TransferChecked};

pub mod constants;
pub mod contexts;
pub mod errors;
pub mod guard;
pub mod merkle;
pub mod sharing;
pub mod state;
pub mod venue;

use constants::*;
use contexts::*;
use errors::CodedError;
use state::*;

declare_id!("Fg6PaFpoGXkYsidMpWTK6W2BeZ7FEfcYkg476zPFsLnS");

// ============================================================== helpers

fn now() -> Result<i64> {
    Ok(Clock::get()?.unix_timestamp)
}

fn mul_bps(amount: u64, bps: u64) -> Result<u64> {
    Ok(((amount as u128)
        .checked_mul(bps as u128)
        .ok_or(CodedError::MathOverflow)?
        / BPS as u128) as u64)
}

fn check_wsol_key(wsol: &AccountInfo, vault: &Pubkey) -> Result<()> {
    let expected = get_associated_token_address_with_program_id(vault, &WSOL_MINT, &SPL_TOKEN_ID);
    require_keys_eq!(*wsol.key, expected, CodedError::BadWsolAccount);
    Ok(())
}

/// Lamports above the rent reserve plus any wrapped SOL.
fn vault_total(vault: &AccountInfo, wsol: &AccountInfo) -> Result<u64> {
    let lamports = vault.lamports().saturating_sub(VAULT_RENT_RESERVE);
    let wrapped = if *wsol.owner == SPL_TOKEN_ID && !wsol.data_is_empty() {
        venue::token_amount(wsol)?
    } else {
        0
    };
    lamports.checked_add(wrapped).ok_or(error!(CodedError::MathOverflow))
}

fn vault_pay<'info>(
    system: &AccountInfo<'info>,
    vault: &AccountInfo<'info>,
    to: &AccountInfo<'info>,
    amount: u64,
    seeds: &[&[&[u8]]],
) -> Result<()> {
    require!(
        vault.lamports().saturating_sub(VAULT_RENT_RESERVE) >= amount,
        CodedError::VaultLamportsLow
    );
    system_program::transfer(
        CpiContext::new_with_signer(
            system.clone(),
            SysTransfer { from: vault.clone(), to: to.clone() },
            seeds,
        ),
        amount,
    )
}

/// Move lamports from the vault into its WSOL account and sync.
fn wrap<'info>(
    system: &AccountInfo<'info>,
    spl_token: &AccountInfo<'info>,
    vault: &AccountInfo<'info>,
    wsol: &AccountInfo<'info>,
    amount: u64,
    seeds: &[&[&[u8]]],
) -> Result<()> {
    require!(
        *wsol.owner == SPL_TOKEN_ID && !wsol.data_is_empty(),
        CodedError::BadWsolAccount
    );
    let lamports_free = vault.lamports().saturating_sub(VAULT_RENT_RESERVE);
    let need = amount.min(lamports_free);
    if need > 0 {
        vault_pay(system, vault, wsol, need, seeds)?;
    }
    spl::sync_native(CpiContext::new(spl_token.clone(), SyncNative { account: wsol.clone() }))
}

#[allow(clippy::too_many_arguments)]
fn fill_router(
    r: &mut Router,
    mint: Pubkey,
    authority: Pubkey,
    token_program: Pubkey,
    config: RouteConfig,
    ref_price: u128,
    now: i64,
    bump: u8,
    vault_bump: u8,
    protocol_fee_bps: u16,
    protocol_vault: Pubkey,
) {
    r.mint = mint;
    r.authority = authority;
    r.token_program = token_program;
    r.config = config;
    r.pending = None;
    r.pending_eta = 0;
    r.holders_bucket = 0;
    r.lp_bucket = 0;
    r.burn_bucket = 0;
    r.reserved_claims = 0;
    r.lp_token_inventory = 0;
    r.lp_mint = Pubkey::default();
    r.lp_locked_amount = 0;
    r.lp_locked_until = 0;
    r.ref_price_q32 = ref_price;
    r.last_observe_ts = now;
    r.last_inflow_ts = 0;
    r.next_epoch = 0;
    r.total_inflow = 0;
    r.total_tokens_burned = 0;
    r.total_lp_quote_added = 0;
    r.total_paid_holders = 0;
    r.bump = bump;
    r.vault_bump = vault_bump;
    r.protocol_fee_bps = protocol_fee_bps;
    r.protocol_vault = protocol_vault;
    r.protocol_verified = protocol_fee_bps == 0;
}

fn fund_vault<'info>(
    system: &AccountInfo<'info>,
    payer: &AccountInfo<'info>,
    vault: &AccountInfo<'info>,
) -> Result<()> {
    let have = vault.lamports();
    if have < VAULT_RENT_RESERVE {
        system_program::transfer(
            CpiContext::new(system.clone(), SysTransfer { from: payer.clone(), to: vault.clone() }),
            VAULT_RENT_RESERVE - have,
        )?;
    }
    Ok(())
}

fn haircut_bps(slippage: u16, fee_allowance: u16) -> u64 {
    BPS.saturating_sub(slippage as u64).saturating_sub(fee_allowance as u64)
}

// ============================================================== program

#[program]
pub mod coded_router {
    use super::*;

    // ---------------------------------------------------------- global

    pub fn init_global(
        ctx: Context<InitGlobal>,
        root_poster: Pubkey,
        crank_tip_bps: u16,
        fee_allowance_bps: u16,
    ) -> Result<()> {
        require!(crank_tip_bps <= MAX_CRANK_TIP_BPS, CodedError::BadParam);
        require!(fee_allowance_bps <= MAX_FEE_ALLOWANCE_BPS, CodedError::BadParam);
        let g = &mut ctx.accounts.global;
        g.admin = ctx.accounts.admin.key();
        g.root_poster = root_poster;
        g.crank_tip_bps = crank_tip_bps;
        g.fee_allowance_bps = fee_allowance_bps;
        g.paused = false;
        g.allowed = Vec::new();
        g.bump = ctx.bumps.global;
        g.protocol_mint = Pubkey::default();
        g.protocol_router = Pubkey::default();
        g.protocol_vault = Pubkey::default();
        g.protocol_fee_bps = 0;
        Ok(())
    }

    pub fn update_global(
        ctx: Context<AdminOnly>,
        new_admin: Pubkey,
        root_poster: Pubkey,
        crank_tip_bps: u16,
        fee_allowance_bps: u16,
        paused: bool,
    ) -> Result<()> {
        require!(crank_tip_bps <= MAX_CRANK_TIP_BPS, CodedError::BadParam);
        require!(fee_allowance_bps <= MAX_FEE_ALLOWANCE_BPS, CodedError::BadParam);
        let g = &mut ctx.accounts.global;
        g.admin = new_admin;
        g.root_poster = root_poster;
        g.crank_tip_bps = crank_tip_bps;
        g.fee_allowance_bps = fee_allowance_bps;
        g.paused = paused;
        Ok(())
    }

    /// Replace the CPI allowlist. Only pump.fun's two programs may appear.
    pub fn set_allowlist(ctx: Context<AdminOnly>, allowed: Vec<AllowedCpi>) -> Result<()> {
        require!(allowed.len() <= MAX_ALLOWED_CPIS, CodedError::AllowlistFull);
        for a in &allowed {
            require!(
                a.program_id == PUMP_PROGRAM_ID || a.program_id == PUMP_AMM_PROGRAM_ID,
                CodedError::CpiNotAllowed
            );
            require!(
                a.kind == CPI_KIND_SWAP || a.kind == CPI_KIND_DEPOSIT,
                CodedError::BadParam
            );
        }
        ctx.accounts.global.allowed = allowed;
        Ok(())
    }

    // ---------------------------------------------------------- router setup

    /// Creates the router and its vault, and seeds the reference price from
    /// the coin's current venue. `venue` = 0 bonding curve, 1 PumpSwap.
    pub fn initialize_router(
        ctx: Context<InitializeRouter>,
        config: RouteConfig,
        venue: u8,
    ) -> Result<()> {
        require!(!ctx.accounts.global.paused, CodedError::Paused);
        config.validate()?;

        let mint_key = ctx.accounts.mint.key();
        let token_program = *ctx.accounts.mint.to_account_info().owner;
        let reserves = venue::read_reserves(
            venue,
            &ctx.accounts.venue_a,
            &ctx.accounts.venue_b,
            &ctx.accounts.venue_c,
            &mint_key,
            &token_program,
        )?;

        fund_vault(
            &ctx.accounts.system_program.to_account_info(),
            &ctx.accounts.authority.to_account_info(),
            &ctx.accounts.vault.to_account_info(),
        )?;

        // Every router created while the protocol fee is on owes it, and
        // can't route anything until verify_protocol_share passes.
        let (fee_bps, fee_vault) = {
            let g = &ctx.accounts.global;
            if g.protocol_vault != Pubkey::default() && g.protocol_fee_bps > 0 {
                (g.protocol_fee_bps, g.protocol_vault)
            } else {
                (0, Pubkey::default())
            }
        };

        let t = now()?;
        let authority = ctx.accounts.authority.key();
        let (bump, vault_bump) = (ctx.bumps.router, ctx.bumps.vault);
        let r = &mut ctx.accounts.router;
        fill_router(
            r,
            mint_key,
            authority,
            token_program,
            config,
            reserves.price_q32()?,
            t,
            bump,
            vault_bump,
            fee_bps,
            fee_vault,
        );

        emit!(RouterCreated {
            router: r.key(),
            mint: mint_key,
            authority: r.authority,
            vault: ctx.accounts.vault.key(),
            config,
        });
        Ok(())
    }

    // ---------------------------------------------------------- protocol

    /// Creates the router for the protocol token. Configure it 100% burn.
    /// It owes no protocol fee itself.
    pub fn init_protocol_router(
        ctx: Context<InitProtocolRouter>,
        config: RouteConfig,
        venue: u8,
    ) -> Result<()> {
        config.validate()?;
        let mint_key = ctx.accounts.mint.key();
        let token_program = *ctx.accounts.mint.to_account_info().owner;
        let reserves = venue::read_reserves(
            venue,
            &ctx.accounts.venue_a,
            &ctx.accounts.venue_b,
            &ctx.accounts.venue_c,
            &mint_key,
            &token_program,
        )?;
        fund_vault(
            &ctx.accounts.system_program.to_account_info(),
            &ctx.accounts.admin.to_account_info(),
            &ctx.accounts.vault.to_account_info(),
        )?;
        let t = now()?;
        let admin = ctx.accounts.admin.key();
        let (bump, vault_bump) = (ctx.bumps.router, ctx.bumps.vault);
        let r = &mut ctx.accounts.router;
        fill_router(
            r,
            mint_key,
            admin,
            token_program,
            config,
            reserves.price_q32()?,
            t,
            bump,
            vault_bump,
            0,
            Pubkey::default(),
        );
        emit!(RouterCreated {
            router: r.key(),
            mint: mint_key,
            authority: admin,
            vault: ctx.accounts.vault.key(),
            config,
        });
        Ok(())
    }

    /// Points the protocol fee at the protocol router and sets its size.
    /// Applies to routers created afterwards; existing ones keep the terms
    /// they were created with. `fee_bps` = 0 turns the fee off.
    pub fn set_protocol(ctx: Context<SetProtocol>, fee_bps: u16) -> Result<()> {
        require!(fee_bps <= MAX_PROTOCOL_FEE_BPS, CodedError::BadParam);
        let router_key = ctx.accounts.protocol_router.key();
        let (vault, _) =
            Pubkey::find_program_address(&[VAULT_SEED, router_key.as_ref()], &crate::ID);
        let g = &mut ctx.accounts.global;
        g.protocol_mint = ctx.accounts.protocol_router.mint;
        g.protocol_router = router_key;
        g.protocol_vault = vault;
        g.protocol_fee_bps = fee_bps;
        emit!(ProtocolSet { mint: g.protocol_mint, router: router_key, vault, fee_bps });
        Ok(())
    }

    /// Permissionless. Reads the coin's locked pump.fun fee split and marks
    /// the router verified if it pays the protocol vault at least its share.
    pub fn verify_protocol_share(ctx: Context<VerifyProtocolShare>) -> Result<()> {
        let r = &mut ctx.accounts.router;
        require!(!r.protocol_verified && r.protocol_fee_bps > 0, CodedError::ProtocolNotRequired);

        let sc = &ctx.accounts.sharing_config;
        let (expected, _) = Pubkey::find_program_address(
            &[PUMP_SHARING_CONFIG_SEED, r.mint.as_ref()],
            &PUMP_FEE_PROGRAM_ID,
        );
        require_keys_eq!(*sc.key, expected, CodedError::BadSharingConfig);
        require_keys_eq!(*sc.owner, PUMP_FEE_PROGRAM_ID, CodedError::BadSharingConfig);

        let paid = sharing::paid_to(&sc.try_borrow_data()?, &r.mint, &r.protocol_vault)?;
        require!(paid >= r.protocol_fee_bps as u32, CodedError::ProtocolShareMissing);

        r.protocol_verified = true;
        emit!(ProtocolVerified { router: r.key(), share_bps: paid as u16 });
        Ok(())
    }

    // ---------------------------------------------------------- config

    pub fn propose_config(ctx: Context<AuthorityOnly>, config: RouteConfig) -> Result<()> {
        config.validate()?;
        let r = &mut ctx.accounts.router;
        r.pending = Some(config);
        r.pending_eta = now()? + CONFIG_TIMELOCK_SECS;
        emit!(ConfigProposed { router: r.key(), config, eta: r.pending_eta });
        Ok(())
    }

    pub fn cancel_config(ctx: Context<AuthorityOnly>) -> Result<()> {
        let r = &mut ctx.accounts.router;
        require!(r.pending.is_some(), CodedError::NoPendingConfig);
        r.pending = None;
        r.pending_eta = 0;
        emit!(ConfigCancelled { router: r.key() });
        Ok(())
    }

    /// Permissionless once the timelock has passed.
    pub fn apply_config(ctx: Context<ApplyConfig>) -> Result<()> {
        let r = &mut ctx.accounts.router;
        let pending = r.pending.ok_or(CodedError::NoPendingConfig)?;
        require!(now()? >= r.pending_eta, CodedError::TimelockActive);
        r.config = pending;
        r.pending = None;
        r.pending_eta = 0;
        emit!(ConfigApplied { router: r.key(), config: pending });
        Ok(())
    }

    /// Hand authority to someone else, or pass Pubkey::default() to renounce
    /// it and make the route config permanent.
    pub fn set_authority(ctx: Context<AuthorityOnly>, new_authority: Pubkey) -> Result<()> {
        let r = &mut ctx.accounts.router;
        r.authority = new_authority;
        if new_authority == Pubkey::default() {
            r.pending = None;
            r.pending_eta = 0;
        }
        emit!(AuthorityChanged { router: r.key(), authority: new_authority });
        Ok(())
    }

    // ---------------------------------------------------------- price

    /// Nudges the reference price 25% toward spot. Rate limited.
    pub fn observe_price(ctx: Context<ObservePrice>, venue: u8) -> Result<()> {
        let t = now()?;
        let r = &mut ctx.accounts.router;
        require!(t >= r.last_observe_ts + OBSERVE_COOLDOWN_SECS, CodedError::ObserveCooldown);
        let res = venue::read_reserves(
            venue,
            &ctx.accounts.venue_a,
            &ctx.accounts.venue_b,
            &ctx.accounts.venue_c,
            &r.mint,
            &r.token_program,
        )?;
        let spot = res.price_q32()?;
        r.ref_price_q32 = if r.ref_price_q32 == 0 {
            spot
        } else {
            (r.ref_price_q32.saturating_mul(3).saturating_add(spot)) / 4
        };
        r.last_observe_ts = t;
        Ok(())
    }

    // ---------------------------------------------------------- inflows

    /// Splits any new SOL in the vault into the route buckets. Call this in
    /// the same transaction as pump's `transfer_creator_fees_to_pump_v2` and
    /// `distribute_creator_fees_v2`.
    pub fn account_inflows(ctx: Context<AccountInflows>) -> Result<()> {
        let g = &ctx.accounts.global;
        require!(!g.paused, CodedError::Paused);
        let vault_info = ctx.accounts.vault.to_account_info();
        check_wsol_key(&ctx.accounts.vault_wsol, vault_info.key)?;

        let t = now()?;
        {
            let r = &ctx.accounts.router;
            require!(r.protocol_verified, CodedError::ProtocolNotVerified);
            require!(
                t >= r.last_inflow_ts + r.config.claim_interval_secs,
                CodedError::TooSoon
            );
        }

        let total = vault_total(&vault_info, &ctx.accounts.vault_wsol)?;
        let accounted = ctx.accounts.router.accounted()?;
        let new = total.saturating_sub(accounted);

        let tip = mul_bps(new, g.crank_tip_bps as u64)?;
        if tip > 0 {
            let router_key = ctx.accounts.router.key();
            let bump = [ctx.accounts.router.vault_bump];
            let seeds: &[&[u8]] = &[VAULT_SEED, router_key.as_ref(), &bump];
            vault_pay(
                &ctx.accounts.system_program.to_account_info(),
                &vault_info,
                &ctx.accounts.cranker.to_account_info(),
                tip,
                &[seeds],
            )?;
        }
        let net = new - tip;

        let r = &mut ctx.accounts.router;
        let h = mul_bps(net, r.config.holders_bps as u64)?;
        let l = mul_bps(net, r.config.lp_bps as u64)?;
        let b = net - h - l;
        r.holders_bucket = r.holders_bucket.checked_add(h).ok_or(CodedError::MathOverflow)?;
        r.lp_bucket = r.lp_bucket.checked_add(l).ok_or(CodedError::MathOverflow)?;
        r.burn_bucket = r.burn_bucket.checked_add(b).ok_or(CodedError::MathOverflow)?;
        r.total_inflow = r.total_inflow.checked_add(net).ok_or(CodedError::MathOverflow)?;
        r.last_inflow_ts = t;

        emit!(InflowAccounted { router: r.key(), amount: net, tip, holders: h, lp: l, burn: b });
        Ok(())
    }

    // ---------------------------------------------------------- buy

    /// Buys the token with SOL from the burn bucket (`purpose` = 0, then
    /// burns it) or from the LP bucket (`purpose` = 1, keeps it for a
    /// deposit). `payload` is a pump `buy`/`buy_v2` or PumpSwap `buy`
    /// instruction built with the vault as the user. Remaining accounts:
    /// [target program, ...instruction accounts].
    pub fn execute_buy<'info>(
        ctx: Context<'_, '_, 'info, 'info, ExecuteBuy<'info>>,
        purpose: u8,
        venue: u8,
        amount_in: u64,
        payload: Vec<u8>,
    ) -> Result<()> {
        let g = &ctx.accounts.global;
        require!(!g.paused, CodedError::Paused);
        require!(amount_in > 0, CodedError::ZeroAmount);

        let vault_info = ctx.accounts.vault.to_account_info();
        check_wsol_key(&ctx.accounts.vault_wsol, vault_info.key)?;

        let (budget, slippage, ref_price, mint_key, token_program) = {
            let r = &ctx.accounts.router;
            let budget = match purpose {
                PURPOSE_BURN => r.burn_bucket,
                PURPOSE_LP => {
                    require!(venue == VENUE_AMM, CodedError::BadVenue);
                    r.lp_bucket / 2
                }
                _ => return err!(CodedError::BadParam),
            };
            (budget, r.config.max_slippage_bps, r.ref_price_q32, r.mint, r.token_program)
        };
        require!(amount_in <= budget, CodedError::ExceedsBucket);

        // Price checks.
        let res = venue::read_reserves(
            venue,
            &ctx.accounts.venue_a,
            &ctx.accounts.venue_b,
            &ctx.accounts.venue_c,
            &mint_key,
            &token_program,
        )?;
        venue::check_deviation(ref_price, res.price_q32()?, slippage)?;
        require!(
            amount_in <= mul_bps(res.quote, slippage as u64)?,
            CodedError::TradeTooLarge
        );
        let min_out = mul_bps(res.tokens_out(amount_in)?, haircut_bps(slippage, g.fee_allowance_bps))?;
        require!(min_out > 0, CodedError::InsufficientOutput);

        let router_key = ctx.accounts.router.key();
        let bump = [ctx.accounts.router.vault_bump];
        let seeds: &[&[u8]] = &[VAULT_SEED, router_key.as_ref(), &bump];
        let signer = &[seeds];

        if venue == VENUE_AMM {
            wrap(
                &ctx.accounts.system_program.to_account_info(),
                &ctx.accounts.spl_token_program.to_account_info(),
                &vault_info,
                &ctx.accounts.vault_wsol.to_account_info(),
                amount_in,
                signer,
            )?;
        }

        let pre_total = vault_total(&vault_info, &ctx.accounts.vault_wsol)?;
        let pre_tokens = ctx.accounts.vault_token_account.amount;

        guard::guarded_invoke(g, CPI_KIND_SWAP, ctx.remaining_accounts, &payload, vault_info.key, signer)?;

        ctx.accounts.vault_token_account.reload()?;
        let post_total = vault_total(&vault_info, &ctx.accounts.vault_wsol)?;
        let post_tokens = ctx.accounts.vault_token_account.amount;

        let spent = pre_total.saturating_sub(post_total);
        require!(spent <= amount_in, CodedError::Overspent);
        let received = post_tokens
            .checked_sub(pre_tokens)
            .ok_or(CodedError::InsufficientOutput)?;
        require!(received >= min_out, CodedError::InsufficientOutput);

        if purpose == PURPOSE_BURN {
            ti::burn(
                CpiContext::new_with_signer(
                    ctx.accounts.token_program.to_account_info(),
                    Burn {
                        mint: ctx.accounts.mint.to_account_info(),
                        from: ctx.accounts.vault_token_account.to_account_info(),
                        authority: vault_info.clone(),
                    },
                    signer,
                ),
                received,
            )?;
        }

        let r = &mut ctx.accounts.router;
        if purpose == PURPOSE_BURN {
            r.burn_bucket -= spent;
            r.total_tokens_burned = r.total_tokens_burned.saturating_add(received);
        } else {
            r.lp_bucket -= spent;
            r.lp_token_inventory = r.lp_token_inventory.saturating_add(received);
        }

        emit!(Bought { router: r.key(), purpose, venue, spent, received, burned: purpose == PURPOSE_BURN });
        Ok(())
    }

    // ---------------------------------------------------------- deposit

    /// Deposits bought tokens plus SOL from the LP bucket into the canonical
    /// PumpSwap pool, then burns or locks the LP tokens. `payload` is a
    /// PumpSwap `deposit` built with the vault as the user.
    pub fn execute_deposit<'info>(
        ctx: Context<'_, '_, 'info, 'info, ExecuteDeposit<'info>>,
        token_in_max: u64,
        quote_in_max: u64,
        payload: Vec<u8>,
    ) -> Result<()> {
        let g = &ctx.accounts.global;
        require!(!g.paused, CodedError::Paused);
        require!(token_in_max > 0 && quote_in_max > 0, CodedError::ZeroAmount);

        let vault_info = ctx.accounts.vault.to_account_info();
        check_wsol_key(&ctx.accounts.vault_wsol, vault_info.key)?;

        let lp_mint_key = ctx.accounts.lp_mint.key();
        let (slippage, ref_price, mint_key, token_program, lp_mode) = {
            let r = &ctx.accounts.router;
            require!(token_in_max <= r.lp_token_inventory, CodedError::ExceedsBucket);
            require!(quote_in_max <= r.lp_bucket, CodedError::ExceedsBucket);
            if r.lp_mint != Pubkey::default() {
                require_keys_eq!(lp_mint_key, r.lp_mint, CodedError::LpMintMismatch);
            }
            (r.config.max_slippage_bps, r.ref_price_q32, r.mint, r.token_program, r.config.lp_mode)
        };
        require!(lp_mint_key != mint_key && lp_mint_key != WSOL_MINT, CodedError::LpMintMismatch);

        let res = venue::amm_reserves(
            &ctx.accounts.pool,
            &ctx.accounts.pool_base,
            &ctx.accounts.pool_quote,
            &mint_key,
            &token_program,
        )?;
        venue::check_deviation(ref_price, res.price_q32()?, slippage)?;

        let router_key = ctx.accounts.router.key();
        let bump = [ctx.accounts.router.vault_bump];
        let seeds: &[&[u8]] = &[VAULT_SEED, router_key.as_ref(), &bump];
        let signer = &[seeds];

        wrap(
            &ctx.accounts.system_program.to_account_info(),
            &ctx.accounts.spl_token_program.to_account_info(),
            &vault_info,
            &ctx.accounts.vault_wsol.to_account_info(),
            quote_in_max,
            signer,
        )?;

        let pre_total = vault_total(&vault_info, &ctx.accounts.vault_wsol)?;
        let pre_tokens = ctx.accounts.vault_token_account.amount;
        let pre_lp = ctx.accounts.vault_lp_account.amount;
        let lp_supply = ctx.accounts.lp_mint.supply;

        guard::guarded_invoke(g, CPI_KIND_DEPOSIT, ctx.remaining_accounts, &payload, vault_info.key, signer)?;

        ctx.accounts.vault_token_account.reload()?;
        ctx.accounts.vault_lp_account.reload()?;
        let post_total = vault_total(&vault_info, &ctx.accounts.vault_wsol)?;

        let token_used = pre_tokens.saturating_sub(ctx.accounts.vault_token_account.amount);
        let quote_used = pre_total.saturating_sub(post_total);
        require!(token_used <= token_in_max, CodedError::Overspent);
        require!(quote_used <= quote_in_max, CodedError::Overspent);
        let lp_received = ctx
            .accounts
            .vault_lp_account
            .amount
            .checked_sub(pre_lp)
            .ok_or(CodedError::InsufficientOutput)?;
        require!(lp_received > 0, CodedError::InsufficientOutput);

        // Fair share of the pool for what we put in, minus slippage.
        if lp_supply > 0 && res.token > 0 && res.quote > 0 {
            let by_token = (token_used as u128 * lp_supply as u128) / res.token as u128;
            let by_quote = (quote_used as u128 * lp_supply as u128) / res.quote as u128;
            let expected = by_token.min(by_quote) as u64;
            require!(
                lp_received >= mul_bps(expected, BPS - slippage as u64)?,
                CodedError::InsufficientOutput
            );
        }

        let t = now()?;
        if lp_mode == LP_MODE_BURN {
            ti::burn(
                CpiContext::new_with_signer(
                    ctx.accounts.lp_token_program.to_account_info(),
                    Burn {
                        mint: ctx.accounts.lp_mint.to_account_info(),
                        from: ctx.accounts.vault_lp_account.to_account_info(),
                        authority: vault_info.clone(),
                    },
                    signer,
                ),
                lp_received,
            )?;
        }

        let r = &mut ctx.accounts.router;
        r.lp_mint = lp_mint_key;
        r.lp_token_inventory -= token_used;
        r.lp_bucket -= quote_used;
        r.total_lp_quote_added = r.total_lp_quote_added.saturating_add(quote_used);
        if lp_mode == LP_MODE_LOCK {
            r.lp_locked_amount = r.lp_locked_amount.saturating_add(lp_received);
            r.lp_locked_until = r.lp_locked_until.max(t + LP_LOCK_SECS);
        }

        emit!(LiquidityAdded {
            router: r.key(),
            token_used,
            quote_used,
            lp_received,
            lp_burned: lp_mode == LP_MODE_BURN,
        });
        Ok(())
    }

    /// After the lock expires, the router authority can take locked LP.
    pub fn release_locked_lp(ctx: Context<ReleaseLockedLp>) -> Result<()> {
        let (amount, until) = (ctx.accounts.router.lp_locked_amount, ctx.accounts.router.lp_locked_until);
        require!(amount > 0, CodedError::ZeroAmount);
        require!(now()? >= until, CodedError::LpLocked);

        let router_key = ctx.accounts.router.key();
        let bump = [ctx.accounts.router.vault_bump];
        let seeds: &[&[u8]] = &[VAULT_SEED, router_key.as_ref(), &bump];
        ti::transfer_checked(
            CpiContext::new_with_signer(
                ctx.accounts.lp_token_program.to_account_info(),
                TransferChecked {
                    from: ctx.accounts.vault_lp_account.to_account_info(),
                    mint: ctx.accounts.lp_mint.to_account_info(),
                    to: ctx.accounts.authority_lp_account.to_account_info(),
                    authority: ctx.accounts.vault.to_account_info(),
                },
                &[seeds],
            ),
            amount,
            ctx.accounts.lp_mint.decimals,
        )?;
        ctx.accounts.router.lp_locked_amount = 0;
        Ok(())
    }

    /// Turns leftover wrapped SOL back into lamports. Permissionless; the
    /// vault's total doesn't change.
    pub fn unwrap_wsol(ctx: Context<UnwrapWsol>) -> Result<()> {
        let vault_info = ctx.accounts.vault.to_account_info();
        check_wsol_key(&ctx.accounts.vault_wsol, vault_info.key)?;
        require!(
            *ctx.accounts.vault_wsol.owner == SPL_TOKEN_ID,
            CodedError::BadWsolAccount
        );
        let router_key = ctx.accounts.router.key();
        let bump = [ctx.accounts.router.vault_bump];
        let seeds: &[&[u8]] = &[VAULT_SEED, router_key.as_ref(), &bump];
        spl::close_account(CpiContext::new_with_signer(
            ctx.accounts.spl_token_program.to_account_info(),
            CloseAccount {
                account: ctx.accounts.vault_wsol.to_account_info(),
                destination: vault_info.clone(),
                authority: vault_info,
            },
            &[seeds],
        ))
    }

    // ---------------------------------------------------------- holders

    /// Posts a merkle root that pays `total` lamports from the holder bucket.
    /// Claims open after a one-hour challenge window.
    pub fn post_epoch(
        ctx: Context<PostEpoch>,
        index: u64,
        root: [u8; 32],
        total: u64,
        num_nodes: u32,
        claim_window_secs: i64,
    ) -> Result<()> {
        require!(!ctx.accounts.global.paused, CodedError::Paused);
        require!(total > 0 && num_nodes > 0, CodedError::ZeroAmount);
        require!(Epoch::bitmap_len(num_nodes) <= MAX_BITMAP_BYTES, CodedError::EpochTooLarge);
        require!(claim_window_secs >= MIN_CLAIM_WINDOW_SECS, CodedError::BadParam);

        let router_key = ctx.accounts.router.key();
        let r = &mut ctx.accounts.router;
        require!(index == r.next_epoch, CodedError::BadEpochIndex);
        require!(total <= r.holders_bucket, CodedError::ExceedsBucket);

        let t = now()?;
        let e = &mut ctx.accounts.epoch;
        e.router = router_key;
        e.index = index;
        e.root = root;
        e.total = total;
        e.claimed = 0;
        e.num_nodes = num_nodes;
        e.claimable_at = t + EPOCH_CHALLENGE_SECS;
        e.expires_at = e.claimable_at + claim_window_secs;
        e.vetoed = false;
        e.poster = ctx.accounts.poster.key();
        e.bump = ctx.bumps.epoch;
        e.bitmap = vec![0u8; Epoch::bitmap_len(num_nodes)];

        r.holders_bucket -= total;
        r.reserved_claims = r.reserved_claims.checked_add(total).ok_or(CodedError::MathOverflow)?;
        r.next_epoch += 1;

        emit!(EpochPosted { router: router_key, index, root, total, num_nodes, claimable_at: e.claimable_at });
        Ok(())
    }

    /// The router authority or protocol admin can reject a root during its
    /// challenge window. Funds go back to the holder bucket.
    pub fn veto_epoch(ctx: Context<VetoEpoch>) -> Result<()> {
        let s = ctx.accounts.signer.key();
        require!(
            s == ctx.accounts.router.authority || s == ctx.accounts.global.admin,
            CodedError::Unauthorized
        );
        let e = &mut ctx.accounts.epoch;
        require!(now()? < e.claimable_at, CodedError::ChallengeOver);
        e.vetoed = true;
        let back = e.total - e.claimed;
        let r = &mut ctx.accounts.router;
        r.reserved_claims -= back;
        r.holders_bucket = r.holders_bucket.checked_add(back).ok_or(CodedError::MathOverflow)?;
        emit!(EpochVetoed { router: r.key(), index: e.index });
        Ok(())
    }

    /// Pays one holder. Anyone can submit it, so the indexer can push
    /// payouts on holders' behalf.
    pub fn claim(ctx: Context<Claim>, index: u32, amount: u64, proof: Vec<[u8; 32]>) -> Result<()> {
        let t = now()?;
        let claimant = &ctx.accounts.claimant;
        let vault_info = ctx.accounts.vault.to_account_info();
        require!(*claimant.owner == system_program::ID, CodedError::BadClaimant);
        require_keys_neq!(*claimant.key, *vault_info.key, CodedError::BadClaimant);

        let epoch_key = ctx.accounts.epoch.key();
        {
            let e = &ctx.accounts.epoch;
            require!(!e.vetoed, CodedError::EpochVetoed);
            require!(t >= e.claimable_at && t < e.expires_at, CodedError::EpochNotClaimable);
            require!(index < e.num_nodes, CodedError::BadIndex);
            require!(!e.is_claimed(index), CodedError::AlreadyClaimed);
            let leaf = merkle::leaf(&epoch_key, index, claimant.key, amount);
            require!(merkle::verify(&proof, &e.root, leaf), CodedError::BadProof);
            require!(
                e.claimed.checked_add(amount).ok_or(CodedError::MathOverflow)? <= e.total,
                CodedError::EpochOverdrawn
            );
        }

        let router_key = ctx.accounts.router.key();
        let bump = [ctx.accounts.router.vault_bump];
        let seeds: &[&[u8]] = &[VAULT_SEED, router_key.as_ref(), &bump];
        vault_pay(
            &ctx.accounts.system_program.to_account_info(),
            &vault_info,
            &claimant.to_account_info(),
            amount,
            &[seeds],
        )?;

        let e = &mut ctx.accounts.epoch;
        e.set_claimed(index);
        e.claimed += amount;
        let r = &mut ctx.accounts.router;
        r.reserved_claims -= amount;
        r.total_paid_holders = r.total_paid_holders.saturating_add(amount);

        emit!(Claimed { router: router_key, epoch: e.index, index, claimant: claimant.key(), amount });
        Ok(())
    }

    /// After expiry, unclaimed SOL returns to the holder bucket and the
    /// epoch account's rent returns to the poster.
    pub fn close_epoch(ctx: Context<CloseEpoch>) -> Result<()> {
        let e = &ctx.accounts.epoch;
        require!(now()? >= e.expires_at, CodedError::EpochNotExpired);
        let back = e.total - e.claimed;
        let r = &mut ctx.accounts.router;
        r.reserved_claims -= back;
        r.holders_bucket = r.holders_bucket.checked_add(back).ok_or(CodedError::MathOverflow)?;
        emit!(EpochClosed { router: r.key(), index: e.index, returned: back });
        Ok(())
    }
}

// ============================================================== events

#[event]
pub struct RouterCreated {
    pub router: Pubkey,
    pub mint: Pubkey,
    pub authority: Pubkey,
    pub vault: Pubkey,
    pub config: RouteConfig,
}

#[event]
pub struct ProtocolSet {
    pub mint: Pubkey,
    pub router: Pubkey,
    pub vault: Pubkey,
    pub fee_bps: u16,
}

#[event]
pub struct ProtocolVerified {
    pub router: Pubkey,
    pub share_bps: u16,
}

#[event]
pub struct ConfigProposed {
    pub router: Pubkey,
    pub config: RouteConfig,
    pub eta: i64,
}

#[event]
pub struct ConfigCancelled {
    pub router: Pubkey,
}

#[event]
pub struct ConfigApplied {
    pub router: Pubkey,
    pub config: RouteConfig,
}

#[event]
pub struct AuthorityChanged {
    pub router: Pubkey,
    pub authority: Pubkey,
}

#[event]
pub struct InflowAccounted {
    pub router: Pubkey,
    pub amount: u64,
    pub tip: u64,
    pub holders: u64,
    pub lp: u64,
    pub burn: u64,
}

#[event]
pub struct Bought {
    pub router: Pubkey,
    pub purpose: u8,
    pub venue: u8,
    pub spent: u64,
    pub received: u64,
    pub burned: bool,
}

#[event]
pub struct LiquidityAdded {
    pub router: Pubkey,
    pub token_used: u64,
    pub quote_used: u64,
    pub lp_received: u64,
    pub lp_burned: bool,
}

#[event]
pub struct EpochPosted {
    pub router: Pubkey,
    pub index: u64,
    pub root: [u8; 32],
    pub total: u64,
    pub num_nodes: u32,
    pub claimable_at: i64,
}

#[event]
pub struct EpochVetoed {
    pub router: Pubkey,
    pub index: u64,
}

#[event]
pub struct Claimed {
    pub router: Pubkey,
    pub epoch: u64,
    pub index: u32,
    pub claimant: Pubkey,
    pub amount: u64,
}

#[event]
pub struct EpochClosed {
    pub router: Pubkey,
    pub index: u64,
    pub returned: u64,
}
