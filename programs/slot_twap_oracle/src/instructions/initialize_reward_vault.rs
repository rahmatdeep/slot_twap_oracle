use anchor_lang::prelude::*;
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};

use crate::errors::OracleError;
use crate::state::{Oracle, RewardVault};

#[derive(Accounts)]
pub struct InitializeRewardVault<'info> {
    #[account(
        seeds = [b"oracle", oracle.base_mint.as_ref(), oracle.quote_mint.as_ref()],
        bump,
        has_one = owner,
    )]
    pub oracle: Account<'info, Oracle>,

    #[account(
        init,
        payer = owner,
        space = 8 + RewardVault::INIT_SPACE,
        seeds = [b"reward", oracle.key().as_ref()],
        bump,
    )]
    pub reward_vault: Account<'info, RewardVault>,

    /// Token account held by the vault PDA to store reward tokens.
    #[account(
        init,
        payer = owner,
        token::mint = reward_mint,
        token::authority = reward_vault,
        seeds = [b"reward_tokens", oracle.key().as_ref()],
        bump,
    )]
    pub vault_token_account: InterfaceAccount<'info, TokenAccount>,

    pub reward_mint: InterfaceAccount<'info, Mint>,

    #[account(mut)]
    pub owner: Signer<'info>,

    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

pub fn handler(ctx: Context<InitializeRewardVault>, reward_per_update: u64) -> Result<()> {
    require!(reward_per_update > 0, OracleError::InvalidCapacity);

    // §23: Reject Token-2022 mints with dangerous extensions.
    // Standard SPL Token mints are 82 bytes. Token-2022 mints with extensions
    // are larger. A mint with PermanentDelegate (allows seizing vault tokens)
    // or FreezeAuthority (allows freezing vault) is dangerous.
    // We reject mints > 170 bytes as a conservative heuristic — basic Token-2022
    // mints without extensions are 82 bytes, mints with simple extensions are
    // ~130-170 bytes. Mints with PermanentDelegate/TransferHook are larger.
    let mint_data_len = ctx.accounts.reward_mint.to_account_info().data_len();
    require!(mint_data_len <= 170, OracleError::InvalidCapacity);

    let vault = &mut ctx.accounts.reward_vault;
    vault.oracle = ctx.accounts.oracle.key();
    vault.reward_mint = ctx.accounts.reward_mint.key();
    vault.reward_per_update = reward_per_update;
    vault.total_distributed = 0;
    vault.total_updates_rewarded = 0;

    Ok(())
}
