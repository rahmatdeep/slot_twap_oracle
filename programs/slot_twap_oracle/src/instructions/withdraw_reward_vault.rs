use anchor_lang::prelude::*;
use anchor_spl::token_interface::{self, TokenAccount, TokenInterface, TransferChecked, Mint};

use crate::state::{Oracle, RewardVault};

#[derive(Accounts)]
pub struct WithdrawRewardVault<'info> {
    #[account(
        seeds = [b"oracle", oracle.base_mint.as_ref(), oracle.quote_mint.as_ref()],
        bump,
        has_one = owner,
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

    #[account(
        mut,
        token::mint = reward_vault.reward_mint,
        constraint = owner_token_account.key() != vault_token_account.key(),
    )]
    pub owner_token_account: InterfaceAccount<'info, TokenAccount>,

    #[account(mut)]
    pub owner: Signer<'info>,

    pub token_program: Interface<'info, TokenInterface>,
}

pub fn handler(ctx: Context<WithdrawRewardVault>, amount: u64) -> Result<()> {
    let oracle_key = ctx.accounts.oracle.key();
    let bump = ctx.accounts.reward_vault.bump;
    let signer_seeds: &[&[&[u8]]] = &[&[b"reward", oracle_key.as_ref(), &[bump]]];

    token_interface::transfer_checked(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            TransferChecked {
                from: ctx.accounts.vault_token_account.to_account_info(),
                to: ctx.accounts.owner_token_account.to_account_info(),
                mint: ctx.accounts.reward_mint.to_account_info(),
                authority: ctx.accounts.reward_vault.to_account_info(),
            },
            signer_seeds,
        ),
        amount,
        ctx.accounts.reward_mint.decimals,
    )?;

    Ok(())
}
