use crate::state::observation::{Observation, ObservationBuffer};

/// Pushes a new observation into the ring buffer, overwriting the oldest entry
/// when capacity is reached.
pub fn push_observation(buffer: &mut ObservationBuffer, slot: u64, cumulative_price: u128) {
    let idx = buffer.head as usize;
    let obs = Observation {
        slot,
        cumulative_price,
    };

    if buffer.observations.len() < buffer.capacity as usize {
        // Buffer not yet full — append
        buffer.observations.push(obs);
    } else {
        // Buffer full — overwrite at head
        buffer.observations[idx] = obs;
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
    let len = buffer.observations.len();
    if len == 0 {
        return None;
    }

    // Start from the entry just before head (the most recently written)
    for i in 1..=len {
        let idx = (buffer.head as usize + len - i) % len;
        let obs = &buffer.observations[idx];
        if obs.slot < target_slot {
            return Some(*obs);
        }
    }

    None
}
