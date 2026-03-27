use anchor_lang::prelude::*;

#[account]
#[derive(InitSpace)]
pub struct Oracle {
    pub base_mint: Pubkey,
    pub quote_mint: Pubkey,
    pub last_price: u128,
    pub cumulative_price: u128,
    pub last_slot: u64,
    pub last_updater: Pubkey,
}
