use anchor_lang::prelude::*;

pub mod errors;
pub mod events;
pub mod instructions;
pub mod math;
pub mod state;
pub mod utils;

use instructions::*;

declare_id!("7LKj9Yk62ddRjtTHvvV6fmquD9h7XbcvKKa7yGtocdsT");

#[program]
pub mod slot_twap_oracle {
    use super::*;

    pub fn initialize_oracle(
        ctx: Context<InitializeOracle>,
        capacity: u32,
    ) -> Result<()> {
        instructions::initialize_oracle::handler(ctx, capacity)
    }

    pub fn update_price(ctx: Context<UpdatePrice>, new_price: u128) -> Result<()> {
        instructions::update_price::handler(ctx, new_price)
    }

    pub fn get_swap(ctx: Context<GetSwap>, window_slots: u64, max_staleness_slots: u64) -> Result<u128> {
        instructions::get_swap::handler(ctx, window_slots, max_staleness_slots)
    }

    pub fn transfer_ownership(ctx: Context<TransferOwnership>) -> Result<()> {
        instructions::transfer_ownership::handler(ctx)
    }

    pub fn set_paused(ctx: Context<SetPaused>, paused: bool) -> Result<()> {
        instructions::set_paused::handler(ctx, paused)
    }
}
