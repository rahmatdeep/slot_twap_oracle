use anchor_lang::prelude::*;

use crate::errors::OracleError;
use crate::events::BufferResized;
use crate::state::observation::Observation;
use crate::state::{ObservationBuffer, Oracle};

#[derive(Accounts)]
#[instruction(new_capacity: u32)]
pub struct ResizeBuffer<'info> {
    #[account(
        seeds = [b"oracle", oracle.base_mint.as_ref(), oracle.quote_mint.as_ref()],
        bump,
        has_one = owner,
    )]
    pub oracle: Account<'info, Oracle>,

    #[account(
        mut,
        has_one = oracle,
        seeds = [b"observation", oracle.key().as_ref()],
        bump,
        realloc = ObservationBuffer::space(new_capacity),
        realloc::payer = owner,
        realloc::zero = false,
    )]
    pub observation_buffer: Account<'info, ObservationBuffer>,

    #[account(mut)]
    pub owner: Signer<'info>,

    pub system_program: Program<'info, System>,
}

pub fn handler(ctx: Context<ResizeBuffer>, new_capacity: u32) -> Result<()> {
    require!(new_capacity > 0, OracleError::InvalidCapacity);

    let buffer = &mut ctx.accounts.observation_buffer;
    let old_capacity = buffer.capacity;
    let populated = buffer.populated();

    if new_capacity < old_capacity && populated > 0 {
        // Shrinking: linearize the ring in chronological order, keep newest.
        let head = buffer.head as usize;
        let cap = old_capacity as usize;
        let mut ordered = Vec::with_capacity(populated);

        if populated < cap {
            // Not yet full: entries are at indices 0..populated in order
            ordered.extend_from_slice(&buffer.observations[..populated]);
        } else {
            // Full and wrapped: head is the oldest
            ordered.extend_from_slice(&buffer.observations[head..cap]);
            ordered.extend_from_slice(&buffer.observations[..head]);
        }

        let keep = (new_capacity as usize).min(ordered.len());
        let start = ordered.len() - keep;
        let kept = &ordered[start..];

        // Build new fixed-size array
        let mut new_obs = vec![Observation::default(); new_capacity as usize];
        new_obs[..keep].copy_from_slice(kept);

        buffer.observations = new_obs;
        buffer.len = keep as u32;
        buffer.head = keep as u32 % new_capacity;
    } else if new_capacity > old_capacity {
        // Growing: extend with zeroed entries
        buffer.observations.resize(new_capacity as usize, Observation::default());
    }

    buffer.capacity = new_capacity;

    emit!(BufferResized {
        oracle: ctx.accounts.oracle.key(),
        old_capacity,
        new_capacity,
        observations_retained: buffer.len,
    });

    Ok(())
}
