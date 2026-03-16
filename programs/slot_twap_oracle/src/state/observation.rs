use anchor_lang::prelude::*;

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Default, InitSpace)]
pub struct Observation {
    pub slot: u64,
    pub cumulative_price: u128,
}

#[account]
pub struct ObservationBuffer {
    pub oracle: Pubkey,
    pub head: u32,
    pub capacity: u32,
    pub observations: Vec<Observation>,
}

impl ObservationBuffer {
    pub fn space(capacity: u32) -> usize {
        8  // discriminator
        + 32 // oracle pubkey
        + 4  // head
        + 4  // capacity
        + 4  // vec length prefix
        + (capacity as usize) * Observation::INIT_SPACE
    }
}
