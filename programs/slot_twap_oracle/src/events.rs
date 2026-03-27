use anchor_lang::prelude::*;

#[event]
pub struct OracleUpdate {
    pub oracle: Pubkey,
    pub price: u128,
    pub cumulative_price: u128,
    pub slot: u64,
    pub updater: Pubkey,
}

#[event]
pub struct OwnershipTransferred {
    pub oracle: Pubkey,
    pub previous_owner: Pubkey,
    pub new_owner: Pubkey,
}

#[event]
pub struct OraclePauseToggled {
    pub oracle: Pubkey,
    pub paused: bool,
}
