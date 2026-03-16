use anchor_lang::prelude::*;

#[error_code]
pub enum OracleError {
    #[msg("Price overflow detected")]
    PriceOverflow,

    #[msg("Stale oracle update — slot has not advanced")]
    StaleSlot,

    #[msg("Not enough observations to compute SWAP for the requested window")]
    InsufficientObservations,

    #[msg("Division by zero — slot span is zero")]
    DivisionByZero,

    #[msg("Observation buffer capacity must be greater than zero")]
    InvalidCapacity,
}
