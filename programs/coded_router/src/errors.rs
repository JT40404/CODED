use anchor_lang::prelude::*;

#[error_code]
pub enum CodedError {
    #[msg("Route percentages must add up to 10000 bps")]
    BadRouteSum,
    #[msg("Slippage is above the allowed maximum")]
    SlippageTooHigh,
    #[msg("Claim interval is shorter than the allowed minimum")]
    IntervalTooShort,
    #[msg("Unknown LP mode")]
    BadLpMode,
    #[msg("Signer is not allowed to do this")]
    Unauthorized,
    #[msg("Protocol is paused")]
    Paused,
    #[msg("No pending config")]
    NoPendingConfig,
    #[msg("Timelock has not expired")]
    TimelockActive,
    #[msg("Claim interval has not elapsed")]
    TooSoon,
    #[msg("Amount exceeds the route bucket")]
    ExceedsBucket,
    #[msg("Amount must be greater than zero")]
    ZeroAmount,
    #[msg("Venue account failed verification")]
    BadVenue,
    #[msg("Bonding curve has completed; use the AMM venue")]
    CurveComplete,
    #[msg("Reference price not initialized; call observe_price first")]
    NoReferencePrice,
    #[msg("Spot price deviates too far from the reference price")]
    PriceDeviation,
    #[msg("Trade is too large for current pool depth")]
    TradeTooLarge,
    #[msg("CPI target or discriminator is not on the allowlist")]
    CpiNotAllowed,
    #[msg("Payload too short")]
    BadPayload,
    #[msg("Spent more than the approved amount")]
    Overspent,
    #[msg("Received less than the minimum output")]
    InsufficientOutput,
    #[msg("Wrapped SOL account is invalid")]
    BadWsolAccount,
    #[msg("LP mint does not match the recorded LP mint")]
    LpMintMismatch,
    #[msg("LP tokens are still locked")]
    LpLocked,
    #[msg("Allowlist is full")]
    AllowlistFull,
    #[msg("Epoch index is out of order")]
    BadEpochIndex,
    #[msg("Epoch too large")]
    EpochTooLarge,
    #[msg("Epoch is not claimable right now")]
    EpochNotClaimable,
    #[msg("Epoch has been vetoed")]
    EpochVetoed,
    #[msg("Challenge window has passed")]
    ChallengeOver,
    #[msg("Epoch has not expired")]
    EpochNotExpired,
    #[msg("Leaf index out of range")]
    BadIndex,
    #[msg("Already claimed")]
    AlreadyClaimed,
    #[msg("Invalid merkle proof")]
    BadProof,
    #[msg("Claim would exceed the epoch total")]
    EpochOverdrawn,
    #[msg("Claimant must be a system account")]
    BadClaimant,
    #[msg("Math overflow")]
    MathOverflow,
    #[msg("Price observation is rate limited")]
    ObserveCooldown,
    #[msg("Vault is missing lamports for this transfer")]
    VaultLamportsLow,
    #[msg("Parameter out of range")]
    BadParam,
    #[msg("This coin's pump.fun fee split doesn't include the protocol share yet; call verify_protocol_share")]
    ProtocolNotVerified,
    #[msg("The pump.fun fee split is missing the protocol share or it is too small")]
    ProtocolShareMissing,
    #[msg("pump.fun sharing config failed verification")]
    BadSharingConfig,
    #[msg("The pump.fun fee split is still editable; lock it before verifying")]
    SharingNotLocked,
    #[msg("Nothing to verify for this router")]
    ProtocolNotRequired,
}
