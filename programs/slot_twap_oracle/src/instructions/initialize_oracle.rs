use anchor_lang::prelude::*;
use anchor_spl::token_interface::Mint;

use crate::errors::OracleError;
use crate::state::{ObservationBuffer, Oracle};

#[derive(Accounts)]
#[instruction(capacity: u32)]
pub struct InitializeOracle<'info> {
    #[account(
        init,
        payer = authority,
        space = 8 + Oracle::INIT_SPACE,
        seeds = [b"oracle", base_mint.key().as_ref(), quote_mint.key().as_ref()],
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

    pub base_mint: InterfaceAccount<'info, Mint>,
    pub quote_mint: InterfaceAccount<'info, Mint>,

    #[account(mut)]
    pub authority: Signer<'info>,

    pub system_program: Program<'info, System>,
}

pub fn handler(
    ctx: Context<InitializeOracle>,
    capacity: u32,
) -> Result<()> {
    require!(capacity > 0, OracleError::InvalidCapacity);

    let oracle = &mut ctx.accounts.oracle;
    let clock = Clock::get()?;

    oracle.owner = ctx.accounts.authority.key();
    oracle.base_mint = ctx.accounts.base_mint.key();
    oracle.quote_mint = ctx.accounts.quote_mint.key();
    oracle.last_price = 0;
    oracle.cumulative_price = 0;
    oracle.last_slot = clock.slot;
    oracle.max_deviation_bps = 1000; // 10% default

    let buffer = &mut ctx.accounts.observation_buffer;
    buffer.oracle = oracle.key();
    buffer.head = 0;
    buffer.len = 0;
    buffer.capacity = capacity;
    buffer.observations = vec![Default::default(); capacity as usize];

    Ok(())
}
