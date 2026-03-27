use anchor_lang::prelude::*;

use crate::errors::OracleError;
use crate::math::compute_swap;
use crate::state::{ObservationBuffer, Oracle};
use crate::utils::get_observation_before_slot;

#[derive(Accounts)]
pub struct GetSwap<'info> {
    #[account(
        seeds = [b"oracle", oracle.base_mint.as_ref(), oracle.quote_mint.as_ref()],
        bump,
    )]
    pub oracle: Account<'info, Oracle>,

    #[account(
        has_one = oracle,
        seeds = [b"observation", oracle.key().as_ref()],
        bump,
    )]
    pub observation_buffer: Account<'info, ObservationBuffer>,
}

pub fn handler(ctx: Context<GetSwap>, window_slots: u64, max_staleness_slots: u64) -> Result<u128> {
    let oracle = &ctx.accounts.oracle;
    let buffer = &ctx.accounts.observation_buffer;
    let clock = Clock::get()?;
    let current_slot = clock.slot;

    require!(!oracle.paused, OracleError::OraclePaused);

    require!(buffer.populated() > 0, OracleError::InsufficientHistory);

    // The cumulative_price on the oracle may have advanced beyond the last
    // stored observation if slots have elapsed since the last update_price.
    // Use the oracle's live cumulative value extended to the current slot.
    let slot_delta_since_last = current_slot
        .checked_sub(oracle.last_slot)
        .ok_or(OracleError::PriceOverflow)?;

    require!(slot_delta_since_last <= max_staleness_slots, OracleError::StaleOracle);

    let cumulative_now = oracle
        .cumulative_price
        .checked_add(
            (oracle.last_price)
                .checked_mul(slot_delta_since_last as u128)
                .ok_or(OracleError::PriceOverflow)?,
        )
        .ok_or(OracleError::PriceOverflow)?;

    // Find an observation before the start of the window
    let window_start = current_slot
        .checked_sub(window_slots)
        .ok_or(OracleError::InsufficientHistory)?;

    let past_obs = get_observation_before_slot(buffer, window_start + 1)
        .ok_or(OracleError::InsufficientHistory)?;

    compute_swap(
        cumulative_now,
        past_obs.cumulative_price,
        current_slot,
        past_obs.slot,
    )
}
