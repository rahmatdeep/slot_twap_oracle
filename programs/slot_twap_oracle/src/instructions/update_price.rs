use anchor_lang::prelude::*;

use crate::errors::OracleError;
use crate::events::PriceUpdated;
use crate::state::{ObservationBuffer, Oracle};
use crate::utils::push_observation;

#[derive(Accounts)]
pub struct UpdatePrice<'info> {
    pub authority: Signer<'info>,

    #[account(
        mut,
        has_one = authority,
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

pub fn handler(ctx: Context<UpdatePrice>, new_price: u128) -> Result<()> {
    let oracle = &mut ctx.accounts.oracle;
    let clock = Clock::get()?;
    let current_slot = clock.slot;

    let slot_delta = current_slot
        .checked_sub(oracle.last_slot)
        .ok_or(OracleError::PriceOverflow)?;

    require!(slot_delta > 0, OracleError::StaleSlot);

    let weighted = (oracle.last_price)
        .checked_mul(slot_delta as u128)
        .ok_or(OracleError::PriceOverflow)?;

    oracle.cumulative_price = oracle
        .cumulative_price
        .checked_add(weighted)
        .ok_or(OracleError::PriceOverflow)?;

    oracle.last_price = new_price;
    oracle.last_slot = current_slot;

    let buffer = &mut ctx.accounts.observation_buffer;
    push_observation(buffer, current_slot, oracle.cumulative_price);

    emit!(PriceUpdated {
        slot: current_slot,
        new_price,
        cumulative_price: oracle.cumulative_price,
    });

    Ok(())
}
