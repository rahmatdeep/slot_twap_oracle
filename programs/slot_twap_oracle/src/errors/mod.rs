use anchor_lang::prelude::*;

#[error_code]
pub enum OracleError {
    #[msg("Price overflow detected")]
    PriceOverflow,

    #[msg("Stale oracle update — slot has not advanced")]
    StaleSlot,

    #[msg("Not enough observations to compute SWAP for the requested window")]
    InsufficientObservations,

    #[msg("Observation buffer capacity must be greater than zero")]
    InvalidCapacity,

    #[msg("Not enough observations to compute swap for requested window")]
    InsufficientHistory,
}
