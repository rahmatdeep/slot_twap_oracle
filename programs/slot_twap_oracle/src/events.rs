use anchor_lang::prelude::*;

#[event]
pub struct PriceUpdated {
    pub slot: u64,
    pub new_price: u128,
    pub cumulative_price: u128,
}
