use anchor_lang::prelude::*;

use crate::events::OraclePauseToggled;
use crate::state::Oracle;

#[derive(Accounts)]
pub struct SetPaused<'info> {
    #[account(
        mut,
        seeds = [b"oracle", oracle.base_mint.as_ref(), oracle.quote_mint.as_ref()],
        bump,
        has_one = owner,
    )]
    pub oracle: Account<'info, Oracle>,

    pub owner: Signer<'info>,
}

pub fn handler(ctx: Context<SetPaused>, paused: bool) -> Result<()> {
    let oracle = &mut ctx.accounts.oracle;
    oracle.paused = paused;

    emit!(OraclePauseToggled {
        oracle: oracle.key(),
        paused,
    });

    Ok(())
}
