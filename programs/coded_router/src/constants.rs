use anchor_lang::prelude::*;

// ---------------------------------------------------------------------------
// External programs (from pump-fun/pump-public-docs)
// ---------------------------------------------------------------------------
pub const PUMP_PROGRAM_ID: Pubkey = pubkey!("6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P");
pub const PUMP_AMM_PROGRAM_ID: Pubkey = pubkey!("pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA");
pub const WSOL_MINT: Pubkey = pubkey!("So11111111111111111111111111111111111111112");
pub const PUMP_FEE_PROGRAM_ID: Pubkey = pubkey!("pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ");

// ---------------------------------------------------------------------------
// Seeds
// ---------------------------------------------------------------------------
pub const GLOBAL_SEED: &[u8] = b"global";
pub const ROUTER_SEED: &[u8] = b"router";
pub const VAULT_SEED: &[u8] = b"vault";
pub const EPOCH_SEED: &[u8] = b"epoch";

// pump PDAs we re-derive to verify venue accounts
pub const PUMP_BONDING_CURVE_SEED: &[u8] = b"bonding-curve";
pub const PUMP_POOL_AUTHORITY_SEED: &[u8] = b"pool-authority";
pub const PUMP_AMM_POOL_SEED: &[u8] = b"pool";
pub const CANONICAL_POOL_INDEX: u16 = 0;
pub const PUMP_SHARING_CONFIG_SEED: &[u8] = b"sharing-config";

// ---------------------------------------------------------------------------
// Protocol parameters
// ---------------------------------------------------------------------------
pub const BPS: u64 = 10_000;
/// Delay between proposing and applying a route config change.
pub const CONFIG_TIMELOCK_SECS: i64 = 48 * 60 * 60;
/// Window after a holder epoch is posted during which it can be vetoed.
pub const EPOCH_CHALLENGE_SECS: i64 = 60 * 60;
/// Minimum time holders have to claim an epoch.
pub const MIN_CLAIM_WINDOW_SECS: i64 = 7 * 24 * 60 * 60;
/// How long LP tokens stay locked when lp_mode = Lock.
pub const LP_LOCK_SECS: i64 = 365 * 24 * 60 * 60;
/// Shortest allowed claim interval.
pub const MIN_INTERVAL_SECS: i64 = 15 * 60;
/// Price observations are rate limited so the reference price can't be
/// dragged around inside one block.
pub const OBSERVE_COOLDOWN_SECS: i64 = 5 * 60;
/// Rent-exempt minimum for a 0-byte system account. Kept in the vault forever.
pub const VAULT_RENT_RESERVE: u64 = 890_880;
pub const MAX_ALLOWED_CPIS: usize = 16;
pub const MAX_CRANK_TIP_BPS: u16 = 100; // 1%
pub const MAX_FEE_ALLOWANCE_BPS: u16 = 500; // 5%
pub const MAX_SLIPPAGE_BPS: u16 = 1_000; // 10%
pub const MAX_BITMAP_BYTES: usize = 9_000; // ~72k claimants per epoch

// ---------------------------------------------------------------------------
// Raw layout offsets. These are the only places we read another program's
// account data. Verify against idl/pump.json before every deployment.
// BondingCurve: [disc 8][virtual_token_reserves u64][virtual_quote_reserves u64]
//               [real_token_reserves u64][real_quote_reserves u64]
//               [token_total_supply u64][complete bool][creator Pubkey]...
// ---------------------------------------------------------------------------
pub const BC_VIRTUAL_TOKEN_RESERVES_OFFSET: usize = 8;
pub const BC_VIRTUAL_QUOTE_RESERVES_OFFSET: usize = 16;
pub const BC_COMPLETE_OFFSET: usize = 48;
/// SPL Token / Token-2022 base layouts.
pub const TOKEN_ACCOUNT_AMOUNT_OFFSET: usize = 64;
pub const MINT_SUPPLY_OFFSET: usize = 36;

/// Protocol fee: share of ALL creator fees sent to the protocol router,
/// which buys back and burns the protocol token.
pub const MAX_PROTOCOL_FEE_BPS: u16 = 500; // 5% hard cap

// pump fees program SharingConfig layout (from pump's IDL):
// [disc 8][bump u8][version u8][status u8][mint 32][admin 32]
// [admin_revoked bool][shareholders: u32 len + n * (address 32, share_bps u16)]
pub const SHARING_CONFIG_DISC: [u8; 8] = [216, 74, 9, 0, 56, 140, 93, 75];
pub const SC_MINT_OFFSET: usize = 11;
pub const SC_ADMIN_REVOKED_OFFSET: usize = 75;
pub const SC_SHAREHOLDERS_LEN_OFFSET: usize = 76;
pub const SC_SHAREHOLDERS_OFFSET: usize = 80;
pub const SC_SHAREHOLDER_SIZE: usize = 34;

/// Venue identifiers.
pub const VENUE_BONDING_CURVE: u8 = 0;
pub const VENUE_AMM: u8 = 1;

/// Purpose of a buy.
pub const PURPOSE_BURN: u8 = 0;
pub const PURPOSE_LP: u8 = 1;

/// Allowed CPI kinds.
pub const CPI_KIND_SWAP: u8 = 0;
pub const CPI_KIND_DEPOSIT: u8 = 1;

/// LP handling modes.
pub const LP_MODE_BURN: u8 = 0;
pub const LP_MODE_LOCK: u8 = 1;
