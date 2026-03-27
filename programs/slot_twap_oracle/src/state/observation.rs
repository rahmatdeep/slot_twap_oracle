use anchor_lang::prelude::*;

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Default, InitSpace)]
pub struct Observation {
    pub slot: u64,
    pub cumulative_price: u128,
}

/// Fixed-size ring buffer for oracle observations.
///
/// Uses a pre-allocated array (zeroed at init) instead of a Vec to avoid
/// borsh length-prefix overhead on every serialize/deserialize. The `len`
/// field tracks how many slots have been written (up to `capacity`), and
/// `head` is the index
///  of the next write position.
#[account]
pub struct ObservationBuffer {
    pub oracle: Pubkey,
    pub head: u32,
    pub len: u32,
    pub capacity: u32,
    pub observations: Vec<Observation>,
}

impl ObservationBuffer {
    pub fn space(capacity: u32) -> usize {
        8  // discriminator
        + 32 // oracle pubkey
        + 4  // head
        + 4  // len
        + 4  // capacity
        + 4  // vec length prefix
        + (capacity as usize) * Observation::INIT_SPACE
    }

    /// Number of populated observations (may be less than capacity before full).
    pub fn populated(&self) -> usize {
        self.len as usize
    }
}
