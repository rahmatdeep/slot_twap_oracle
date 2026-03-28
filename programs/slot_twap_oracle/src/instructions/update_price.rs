use anchor_lang::prelude::*;
use anchor_spl::token_interface::{self, TokenAccount, TokenInterface, TransferChecked, Mint};

use crate::errors::OracleError;
use crate::events::{OracleUpdate, RewardClaimed};
use crate::state::{ObservationBuffer, Oracle, RewardVault};
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

    // ── Optional reward accounts ──
    // Pass all four to auto-pay the previous updater on each update.
    // Omit all four for a reward-free update (backwards compatible).

    #[account(
        mut,
        seeds = [b"reward", oracle.key().as_ref()],
        bump,
        has_one = oracle,
    )]
    pub reward_vault: Option<Account<'info, RewardVault>>,

    #[account(
        mut,
        seeds = [b"reward_tokens", oracle.key().as_ref()],
        bump,
    )]
    pub vault_token_account: Option<InterfaceAccount<'info, TokenAccount>>,

    pub reward_mint: Option<InterfaceAccount<'info, Mint>>,

    /// Token account of the *previous* updater to receive the reward.
    #[account(mut)]
    pub previous_updater_token_account: Option<InterfaceAccount<'info, TokenAccount>>,

    pub token_program: Option<Interface<'info, TokenInterface>>,
}

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

    // Reject prices that deviate more than the oracle's configured threshold.
    // Skip the check when last_price is zero (first update).
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
            deviation_bps <= oracle.max_deviation_bps as u128,
            OracleError::PriceDeviationTooLarge
        );
    }

    // ── Auto-reward previous updater ──
    // Only if all reward accounts are provided AND there is a previous updater
    // (last_updater != default means someone has updated before).
    if let (
        Some(reward_vault),
        Some(vault_token_account),
        Some(reward_mint),
        Some(prev_updater_ata),
        Some(token_program),
    ) = (
        &ctx.accounts.reward_vault,
        &ctx.accounts.vault_token_account,
        &ctx.accounts.reward_mint,
        &ctx.accounts.previous_updater_token_account,
        &ctx.accounts.token_program,
    ) {
        let reward_amount = reward_vault.reward_per_update;

        if oracle.last_updater != Pubkey::default()
            && vault_token_account.amount >= reward_amount
            && reward_amount > 0
        {
            let oracle_key = oracle.key();
            let seeds: &[&[u8]] = &[b"reward", oracle_key.as_ref()];
            let (_, bump) = Pubkey::find_program_address(seeds, ctx.program_id);
            let signer_seeds: &[&[&[u8]]] = &[&[b"reward", oracle_key.as_ref(), &[bump]]];

            token_interface::transfer_checked(
                CpiContext::new_with_signer(
                    token_program.to_account_info(),
                    TransferChecked {
                        from: vault_token_account.to_account_info(),
                        to: prev_updater_ata.to_account_info(),
                        mint: reward_mint.to_account_info(),
                        authority: reward_vault.to_account_info(),
                    },
                    signer_seeds,
                ),
                reward_amount,
                reward_mint.decimals,
            )?;

            // Update accounting
            let vault = ctx.accounts.reward_vault.as_mut().unwrap();
            vault.total_distributed = vault.total_distributed.checked_add(reward_amount)
                .ok_or(OracleError::PriceOverflow)?;
            vault.total_updates_rewarded = vault.total_updates_rewarded.checked_add(1)
                .ok_or(OracleError::PriceOverflow)?;

            emit!(RewardClaimed {
                oracle: oracle.key(),
                updater: oracle.last_updater,
                amount: reward_amount,
                total_distributed: vault.total_distributed,
            });
        }
    }

    // ── Core update logic ──

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
