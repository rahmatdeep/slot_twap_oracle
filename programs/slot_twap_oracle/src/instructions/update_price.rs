use anchor_lang::prelude::*;

use crate::errors::OracleError;
use crate::events::OracleUpdate;
use crate::state::{ObservationBuffer, Oracle};
use crate::utils::push_observation;

#[derive(Accounts)]
pub struct UpdatePrice<'info> {
    pub payer: Signer<'info>,

    #[account(
        mut,
        seeds = [b"oracle", oracle.base_mint.as_ref(), oracle.quote_mint.as_ref()],
        bump,
    )]
    pub oracle: Account<'info, Oracle>,

    #[account(
        mut,
        has_one = oracle,
        seeds = [b"observation", oracle.key().as_ref()],
        bump,
    )]
    pub observation_buffer: Account<'info, ObservationBuffer>,
}

/// Maximum allowed price deviation in basis points (10% = 1000 bps).
const MAX_PRICE_DEVIATION_BPS: u128 = 1000;
const BPS_DENOMINATOR: u128 = 10_000;

pub fn handler(ctx: Context<UpdatePrice>, new_price: u128) -> Result<()> {
    let oracle = &mut ctx.accounts.oracle;
    let clock = Clock::get()?;
    let current_slot = clock.slot;

    let slot_delta = current_slot
        .checked_sub(oracle.last_slot)
        .ok_or(OracleError::PriceOverflow)?;

    require!(!oracle.paused, OracleError::OraclePaused);
    require!(slot_delta > 0, OracleError::StaleSlot);

    // Reject prices that deviate more than MAX_PRICE_DEVIATION_BPS from the
    // last known price. Skip the check when last_price is zero (first update).
    if oracle.last_price != 0 {
        let diff = if new_price >= oracle.last_price {
            new_price - oracle.last_price
        } else {
            oracle.last_price - new_price
        };
        let deviation_bps = diff
            .checked_mul(BPS_DENOMINATOR)
            .ok_or(OracleError::PriceOverflow)?
            / oracle.last_price;
        require!(
            deviation_bps <= MAX_PRICE_DEVIATION_BPS,
            OracleError::PriceDeviationTooLarge
        );
    }

    let weighted = (oracle.last_price)
        .checked_mul(slot_delta as u128)
        .ok_or(OracleError::PriceOverflow)?;

    oracle.cumulative_price = oracle
        .cumulative_price
        .checked_add(weighted)
        .ok_or(OracleError::PriceOverflow)?;

    oracle.last_price = new_price;
    oracle.last_slot = current_slot;
    oracle.last_updater = ctx.accounts.payer.key();

    let buffer = &mut ctx.accounts.observation_buffer;
    push_observation(buffer, current_slot, oracle.cumulative_price);

    emit!(OracleUpdate {
        oracle: oracle.key(),
        price: new_price,
        cumulative_price: oracle.cumulative_price,
        slot: current_slot,
        updater: ctx.accounts.payer.key(),
    });

    Ok(())
}
