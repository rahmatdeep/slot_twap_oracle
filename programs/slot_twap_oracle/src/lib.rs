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
        base_mint: Pubkey,
        quote_mint: Pubkey,
        capacity: u32,
    ) -> Result<()> {
        instructions::initialize_oracle::handler(ctx, base_mint, quote_mint, capacity)
    }

    pub fn update_price(ctx: Context<UpdatePrice>, new_price: u128) -> Result<()> {
        instructions::update_price::handler(ctx, new_price)
    }
}
