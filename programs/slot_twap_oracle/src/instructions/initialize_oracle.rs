use anchor_lang::prelude::*;

use crate::state::{ObservationBuffer, Oracle};

#[derive(Accounts)]
#[instruction(base_mint: Pubkey, quote_mint: Pubkey, capacity: u32)]
pub struct InitializeOracle<'info> {
    #[account(
        init,
        payer = authority,
        space = 8 + Oracle::INIT_SPACE,
        seeds = [b"oracle", base_mint.as_ref(), quote_mint.as_ref()],
        bump,
    )]
    pub oracle: Account<'info, Oracle>,

    #[account(
        init,
        payer = authority,
        space = ObservationBuffer::space(capacity),
        seeds = [b"observation", oracle.key().as_ref()],
        bump,
    )]
    pub observation_buffer: Account<'info, ObservationBuffer>,

    #[account(mut)]
    pub authority: Signer<'info>,

    pub system_program: Program<'info, System>,
}

pub fn handler(
    ctx: Context<InitializeOracle>,
    base_mint: Pubkey,
    quote_mint: Pubkey,
    capacity: u32,
) -> Result<()> {
    let oracle = &mut ctx.accounts.oracle;
    let clock = Clock::get()?;

    oracle.authority = ctx.accounts.authority.key();
    oracle.base_mint = base_mint;
    oracle.quote_mint = quote_mint;
    oracle.last_price = 0;
    oracle.cumulative_price = 0;
    oracle.last_slot = clock.slot;
    oracle.bump = ctx.bumps.oracle;

    let buffer = &mut ctx.accounts.observation_buffer;
    buffer.oracle = oracle.key();
    buffer.head = 0;
    buffer.capacity = capacity;
    buffer.observations = Vec::new();

    Ok(())
}
