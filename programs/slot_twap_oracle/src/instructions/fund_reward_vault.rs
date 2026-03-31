use anchor_lang::prelude::*;
use anchor_spl::token_interface::{self, TokenAccount, TokenInterface, TransferChecked, Mint};

use crate::state::{Oracle, RewardVault};

#[derive(Accounts)]
pub struct FundRewardVault<'info> {
    #[account(
        seeds = [b"oracle", oracle.base_mint.as_ref(), oracle.quote_mint.as_ref()],
        bump,
    )]
    pub oracle: Account<'info, Oracle>,

    #[account(
        seeds = [b"reward", oracle.key().as_ref()],
        bump,
        has_one = oracle,
    )]
    pub reward_vault: Account<'info, RewardVault>,

    #[account(
        mut,
        seeds = [b"reward_tokens", oracle.key().as_ref()],
        bump,
    )]
    pub vault_token_account: InterfaceAccount<'info, TokenAccount>,

    #[account(
        address = reward_vault.reward_mint,
    )]
    pub reward_mint: InterfaceAccount<'info, Mint>,

    /// Funder's token account — anyone can fund the vault.
    #[account(
        mut,
        token::mint = reward_vault.reward_mint,
        constraint = funder_token_account.key() != vault_token_account.key(),
    )]
    pub funder_token_account: InterfaceAccount<'info, TokenAccount>,

    #[account(mut)]
    pub funder: Signer<'info>,

    pub token_program: Interface<'info, TokenInterface>,
}

pub fn handler(ctx: Context<FundRewardVault>, amount: u64) -> Result<()> {
    token_interface::transfer_checked(
        CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            TransferChecked {
                from: ctx.accounts.funder_token_account.to_account_info(),
                to: ctx.accounts.vault_token_account.to_account_info(),
                mint: ctx.accounts.reward_mint.to_account_info(),
                authority: ctx.accounts.funder.to_account_info(),
            },
        ),
        amount,
        ctx.accounts.reward_mint.decimals,
    )?;

    Ok(())
}
