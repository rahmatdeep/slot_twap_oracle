use anchor_lang::prelude::*;

#[error_code]
pub enum OracleError {
    #[msg("Price overflow detected")]
    PriceOverflow,

    #[msg("Stale oracle update — slot has not advanced")]
    StaleSlot,

    #[msg("Not enough observations to compute swap for requested window")]
    InsufficientHistory,

    #[msg("Observation buffer capacity must be greater than zero")]
    InvalidCapacity,

    #[msg("Oracle data is stale — last update exceeds max staleness threshold")]
    StaleOracle,

    #[msg("Price deviation from last update exceeds maximum allowed threshold")]
    PriceDeviationTooLarge,

    #[msg("Signer is not the oracle owner")]
    Unauthorized,

    #[msg("Oracle is paused")]
    OraclePaused,
}
