use crate::state::observation::{Observation, ObservationBuffer};

/// Pushes a new observation into the fixed-size ring buffer.
///
/// Before the buffer is full (`len < capacity`), writes sequentially and
/// increments `len`. Once full, overwrites the oldest entry at `head`.
/// `head` always advances and wraps at `capacity`.
pub fn push_observation(buffer: &mut ObservationBuffer, slot: u64, cumulative_price: u128) {
    let idx = buffer.head as usize;
    let obs = Observation {
        slot,
        cumulative_price,
    };

    buffer.observations[idx] = obs;

    if buffer.len < buffer.capacity {
        buffer.len += 1;
    }

    buffer.head = (buffer.head + 1) % buffer.capacity;
}

/// Returns the most recent observation whose slot is strictly less than
/// `target_slot`, or `None` if no such observation exists.
///
/// Scans backwards from the most recent entry so that the newest qualifying
/// observation is found first.
pub fn get_observation_before_slot(
    buffer: &ObservationBuffer,
    target_slot: u64,
) -> Option<Observation> {
    let len = buffer.populated();
    if len == 0 {
        return None;
    }

    // Start from the entry just before head (the most recently written)
    for i in 1..=len {
        let idx = (buffer.head as usize + buffer.capacity as usize - i) % buffer.capacity as usize;
        let obs = &buffer.observations[idx];
        if obs.slot < target_slot {
            return Some(*obs);
        }
    }

    None
}
