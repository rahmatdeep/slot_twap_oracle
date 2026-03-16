use crate::errors::OracleError;
use anchor_lang::prelude::*;

/// Computes the Slot-Weighted Average Price (SWAP) between two oracle snapshots.
///
/// # Slot-weighted pricing
///
/// Each time `update_price` is called, the oracle accumulates:
///
///   cumulative_price += last_price * (current_slot - last_slot)
///
/// This means the cumulative value grows proportionally to both the price and
/// the number of slots it was active. The SWAP over an interval is therefore:
///
///   SWAP = (cumulative_now - cumulative_past) / (slot_now - slot_past)
///
/// This gives the average price weighted by the duration (in slots) each price
/// was held — analogous to a time-weighted average price (TWAP).
///
/// # Integer division and precision
///
/// The final division is integer division — any fractional remainder is
/// truncated, not rounded. For example, a cumulative delta of 7 over 2 slots
/// yields 3, not 3.5. To preserve meaningful precision, price inputs passed
/// to `update_price` should be pre-scaled by a fixed factor (e.g. 1e6 or 1e9).
/// The SWAP result will then carry the same scale, and callers can interpret it
/// accordingly.
///
/// # Why slots instead of timestamps on Solana
///
/// Solana's `Clock::unix_timestamp` is set by validators via a stake-weighted
/// median and can drift, stall, or move non-monotonically across slots. Slots,
/// on the other hand, are the native unit of chain progress — they increment
/// deterministically, never repeat, and are available on-chain at zero cost via
/// the Clock sysvar. Using slots as the time axis makes the TWAP immune to
/// timestamp manipulation and ensures strictly monotonic intervals.
pub fn compute_swap(
    cumulative_now: u128,
    cumulative_past: u128,
    slot_now: u64,
    slot_past: u64,
) -> Result<u128> {
    let slot_delta = slot_now
        .checked_sub(slot_past)
        .ok_or(OracleError::PriceOverflow)?;

    require!(slot_delta > 0, OracleError::StaleSlot);

    let cumulative_delta = cumulative_now
        .checked_sub(cumulative_past)
        .ok_or(OracleError::PriceOverflow)?;

    // Integer division: fractional remainder is truncated, not rounded.
    Ok(cumulative_delta / slot_delta as u128)
}
