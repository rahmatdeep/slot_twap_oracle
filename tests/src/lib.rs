#[cfg(test)]
mod tests {
    use anchor_lang::{AnchorDeserialize, InstructionData};
    use litesvm::LiteSVM;
    use solana_sdk::{
        instruction::{AccountMeta, Instruction},
        pubkey::Pubkey,
        signature::Keypair,
        signer::Signer,
        system_program,
        transaction::Transaction,
    };
    use std::str::FromStr;

    use slot_twap_oracle::math::compute_swap;
    use slot_twap_oracle::state::{ObservationBuffer, Oracle};
    use slot_twap_oracle::utils::get_observation_before_slot;

    const PROGRAM_ID: &str = "7LKj9Yk62ddRjtTHvvV6fmquD9h7XbcvKKa7yGtocdsT";
    const DEFAULT_CAPACITY: u32 = 32;

    fn program_id() -> Pubkey {
        Pubkey::from_str(PROGRAM_ID).unwrap()
    }

    fn oracle_pda(base_mint: &Pubkey, quote_mint: &Pubkey) -> (Pubkey, u8) {
        Pubkey::find_program_address(
            &[b"oracle", base_mint.as_ref(), quote_mint.as_ref()],
            &program_id(),
        )
    }

    fn observation_buffer_pda(oracle: &Pubkey) -> (Pubkey, u8) {
        Pubkey::find_program_address(&[b"observation", oracle.as_ref()], &program_id())
    }

    fn setup() -> LiteSVM {
        let mut svm = LiteSVM::new();
        svm.add_program_from_file(program_id(), "../target/deploy/slot_twap_oracle.so")
            .expect("Failed to load program");
        svm
    }

    fn build_initialize_ix(
        payer: &Pubkey,
        base_mint: &Pubkey,
        quote_mint: &Pubkey,
        capacity: u32,
    ) -> Instruction {
        let (oracle_pda, _) = oracle_pda(base_mint, quote_mint);
        let (obs_pda, _) = observation_buffer_pda(&oracle_pda);

        let data = slot_twap_oracle::instruction::InitializeOracle {
            base_mint: *base_mint,
            quote_mint: *quote_mint,
            capacity,
        }
        .data();

        Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(oracle_pda, false),
                AccountMeta::new(obs_pda, false),
                AccountMeta::new(*payer, true),
                AccountMeta::new_readonly(system_program::id(), false),
            ],
            data,
        }
    }

    fn build_update_price_ix(oracle: &Pubkey, new_price: u128) -> Instruction {
        let (obs_pda, _) = observation_buffer_pda(oracle);
        let data = slot_twap_oracle::instruction::UpdatePrice { new_price }.data();

        Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(*oracle, false),
                AccountMeta::new(obs_pda, false),
            ],
            data,
        }
    }

    fn deserialize_oracle(svm: &LiteSVM, pubkey: &Pubkey) -> Oracle {
        let account = svm.get_account(pubkey).expect("Oracle account not found");
        Oracle::deserialize(&mut &account.data[8..]).expect("Failed to deserialize Oracle")
    }

    fn deserialize_observation_buffer(svm: &LiteSVM, pubkey: &Pubkey) -> ObservationBuffer {
        let account = svm
            .get_account(pubkey)
            .expect("ObservationBuffer account not found");
        ObservationBuffer::deserialize(&mut &account.data[8..])
            .expect("Failed to deserialize ObservationBuffer")
    }

    /// Helper: initialize oracle + observation buffer and return (oracle_pda, init_slot)
    fn init_oracle(
        svm: &mut LiteSVM,
        payer: &Keypair,
        base_mint: &Pubkey,
        quote_mint: &Pubkey,
        capacity: u32,
    ) -> (Pubkey, u64) {
        let (oracle_pda, _) = oracle_pda(base_mint, quote_mint);
        let ix = build_initialize_ix(&payer.pubkey(), base_mint, quote_mint, capacity);
        let blockhash = svm.latest_blockhash();
        let tx =
            Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[payer], blockhash);
        svm.send_transaction(tx).unwrap();
        let init_slot = deserialize_oracle(svm, &oracle_pda).last_slot;
        (oracle_pda, init_slot)
    }

    /// Helper: warp slot, expire blockhash, send update_price
    fn do_update_price(
        svm: &mut LiteSVM,
        payer: &Keypair,
        oracle_pda: &Pubkey,
        new_price: u128,
        target_slot: u64,
    ) {
        svm.warp_to_slot(target_slot);
        svm.expire_blockhash();
        let ix = build_update_price_ix(oracle_pda, new_price);
        let blockhash = svm.latest_blockhash();
        let tx =
            Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[payer], blockhash);
        svm.send_transaction(tx).unwrap();
    }

    // ── Happy-path tests ──

    #[test]
    fn test_initialize_oracle() {
        let mut svm = setup();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();

        let base_mint = Pubkey::new_unique();
        let quote_mint = Pubkey::new_unique();
        let (oracle_pda, _) = init_oracle(&mut svm, &payer, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        let oracle = deserialize_oracle(&svm, &oracle_pda);
        assert_eq!(oracle.base_mint, base_mint);
        assert_eq!(oracle.quote_mint, quote_mint);
        assert_eq!(oracle.last_price, 0);
        assert_eq!(oracle.cumulative_price, 0);

        let (obs_pda, _) = observation_buffer_pda(&oracle_pda);
        let buffer = deserialize_observation_buffer(&svm, &obs_pda);
        assert_eq!(buffer.oracle, oracle_pda);
        assert_eq!(buffer.head, 0);
        assert_eq!(buffer.capacity, DEFAULT_CAPACITY);
        assert_eq!(buffer.observations.len(), 0);
    }

    #[test]
    fn test_update_price_single() {
        let mut svm = setup();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();

        let base_mint = Pubkey::new_unique();
        let quote_mint = Pubkey::new_unique();
        let (oracle_pda, init_slot) =
            init_oracle(&mut svm, &payer, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        let new_price: u128 = 1_000_000;
        do_update_price(&mut svm, &payer, &oracle_pda, new_price, init_slot + 10);

        let oracle = deserialize_oracle(&svm, &oracle_pda);
        assert_eq!(oracle.last_price, new_price);
        assert_eq!(oracle.cumulative_price, 0);
        assert_eq!(oracle.last_slot, init_slot + 10);

        let (obs_pda, _) = observation_buffer_pda(&oracle_pda);
        let buffer = deserialize_observation_buffer(&svm, &obs_pda);
        assert_eq!(buffer.observations.len(), 1);
        assert_eq!(buffer.observations[0].slot, init_slot + 10);
        assert_eq!(buffer.observations[0].cumulative_price, 0);
    }

    #[test]
    fn test_update_price_accumulates_cumulative() {
        let mut svm = setup();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();

        let base_mint = Pubkey::new_unique();
        let quote_mint = Pubkey::new_unique();
        let (oracle_pda, init_slot) =
            init_oracle(&mut svm, &payer, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        // First update: set price to 500, after 10 slots
        do_update_price(&mut svm, &payer, &oracle_pda, 500, init_slot + 10);
        let oracle = deserialize_oracle(&svm, &oracle_pda);
        assert_eq!(oracle.cumulative_price, 0);
        assert_eq!(oracle.last_price, 500);

        // Second update: set price to 1000, after 20 more slots
        do_update_price(&mut svm, &payer, &oracle_pda, 1000, init_slot + 30);
        let oracle = deserialize_oracle(&svm, &oracle_pda);
        assert_eq!(oracle.cumulative_price, 10_000);
        assert_eq!(oracle.last_price, 1000);

        // Third update: set price to 2000, after 5 more slots
        do_update_price(&mut svm, &payer, &oracle_pda, 2000, init_slot + 35);
        let oracle = deserialize_oracle(&svm, &oracle_pda);
        assert_eq!(oracle.cumulative_price, 15_000);
        assert_eq!(oracle.last_price, 2000);
        assert_eq!(oracle.last_slot, init_slot + 35);

        // Verify observations were stored
        let (obs_pda, _) = observation_buffer_pda(&oracle_pda);
        let buffer = deserialize_observation_buffer(&svm, &obs_pda);
        assert_eq!(buffer.observations.len(), 3);
    }

    // ── Observation buffer tests ──

    #[test]
    fn test_observation_buffer_stores_all_updates() {
        let mut svm = setup();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();

        let base_mint = Pubkey::new_unique();
        let quote_mint = Pubkey::new_unique();
        let (oracle_pda, init_slot) =
            init_oracle(&mut svm, &payer, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        do_update_price(&mut svm, &payer, &oracle_pda, 100, init_slot + 5);
        do_update_price(&mut svm, &payer, &oracle_pda, 200, init_slot + 15);
        do_update_price(&mut svm, &payer, &oracle_pda, 300, init_slot + 25);

        let (obs_pda, _) = observation_buffer_pda(&oracle_pda);
        let buffer = deserialize_observation_buffer(&svm, &obs_pda);

        assert_eq!(buffer.observations.len(), 3);
        assert_eq!(buffer.observations[0].slot, init_slot + 5);
        assert_eq!(buffer.observations[0].cumulative_price, 0); // 0 * 5
        assert_eq!(buffer.observations[1].slot, init_slot + 15);
        assert_eq!(buffer.observations[1].cumulative_price, 1_000); // 0 + 100*10
        assert_eq!(buffer.observations[2].slot, init_slot + 25);
        assert_eq!(buffer.observations[2].cumulative_price, 3_000); // 1_000 + 200*10
        assert_eq!(buffer.head, 3);
    }

    #[test]
    fn test_observation_buffer_ring_wraps() {
        let mut svm = setup();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();

        let base_mint = Pubkey::new_unique();
        let quote_mint = Pubkey::new_unique();
        let capacity = 3u32;
        let (oracle_pda, init_slot) =
            init_oracle(&mut svm, &payer, &base_mint, &quote_mint, capacity);

        // Fill the buffer (3 updates = capacity)
        do_update_price(&mut svm, &payer, &oracle_pda, 100, init_slot + 10);
        do_update_price(&mut svm, &payer, &oracle_pda, 200, init_slot + 20);
        do_update_price(&mut svm, &payer, &oracle_pda, 300, init_slot + 30);

        let (obs_pda, _) = observation_buffer_pda(&oracle_pda);
        let buffer = deserialize_observation_buffer(&svm, &obs_pda);
        assert_eq!(buffer.observations.len(), 3);
        assert_eq!(buffer.head, 0); // wrapped around

        // 4th update should overwrite index 0
        do_update_price(&mut svm, &payer, &oracle_pda, 400, init_slot + 40);

        let buffer = deserialize_observation_buffer(&svm, &obs_pda);
        assert_eq!(buffer.observations.len(), 3); // still 3
        assert_eq!(buffer.head, 1);
        // Index 0 was overwritten with the 4th observation
        assert_eq!(buffer.observations[0].slot, init_slot + 40);
        // Index 1 and 2 still hold old entries
        assert_eq!(buffer.observations[1].slot, init_slot + 20);
        assert_eq!(buffer.observations[2].slot, init_slot + 30);
    }

    #[test]
    fn test_get_observation_before_slot() {
        let mut svm = setup();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();

        let base_mint = Pubkey::new_unique();
        let quote_mint = Pubkey::new_unique();
        let (oracle_pda, init_slot) =
            init_oracle(&mut svm, &payer, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        do_update_price(&mut svm, &payer, &oracle_pda, 100, init_slot + 10);
        do_update_price(&mut svm, &payer, &oracle_pda, 200, init_slot + 20);
        do_update_price(&mut svm, &payer, &oracle_pda, 300, init_slot + 30);

        let (obs_pda, _) = observation_buffer_pda(&oracle_pda);
        let buffer = deserialize_observation_buffer(&svm, &obs_pda);

        // Looking for observation before slot init_slot+25 → should be slot init_slot+20
        let obs = get_observation_before_slot(&buffer, init_slot + 25).unwrap();
        assert_eq!(obs.slot, init_slot + 20);

        // Looking for observation before slot init_slot+10 → None (no earlier observation)
        let obs = get_observation_before_slot(&buffer, init_slot + 10);
        assert!(obs.is_none());

        // Looking for observation before slot init_slot+31 → should be slot init_slot+30
        let obs = get_observation_before_slot(&buffer, init_slot + 31).unwrap();
        assert_eq!(obs.slot, init_slot + 30);
    }

    // ── compute_swap tests ──

    #[test]
    fn test_compute_swap_uniform_price() {
        let result = compute_swap(10_000, 0, 20, 0).unwrap();
        assert_eq!(result, 500);
    }

    #[test]
    fn test_compute_swap_two_intervals() {
        let result = compute_swap(10_000, 0, 30, 10).unwrap();
        assert_eq!(result, 500);
    }

    #[test]
    fn test_compute_swap_mixed_prices() {
        let result = compute_swap(11_000, 0, 30, 0).unwrap();
        assert_eq!(result, 366);
    }

    #[test]
    fn test_compute_swap_partial_window() {
        let result = compute_swap(11_000, 1_000, 30, 10).unwrap();
        assert_eq!(result, 500);
    }

    #[test]
    fn test_compute_swap_end_to_end() {
        let mut svm = setup();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();

        let base_mint = Pubkey::new_unique();
        let quote_mint = Pubkey::new_unique();
        let (oracle_pda, init_slot) =
            init_oracle(&mut svm, &payer, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        let snap_slot_past = init_slot;
        let snap_cumulative_past = 0u128;

        do_update_price(&mut svm, &payer, &oracle_pda, 200, init_slot + 10);
        do_update_price(&mut svm, &payer, &oracle_pda, 800, init_slot + 20);
        do_update_price(&mut svm, &payer, &oracle_pda, 600, init_slot + 40);

        let oracle = deserialize_oracle(&svm, &oracle_pda);
        assert_eq!(oracle.cumulative_price, 18_000);

        let swap = compute_swap(
            oracle.cumulative_price,
            snap_cumulative_past,
            oracle.last_slot,
            snap_slot_past,
        )
        .unwrap();
        assert_eq!(swap, 450);
    }

    #[test]
    fn test_compute_swap_from_observation_buffer() {
        let mut svm = setup();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();

        let base_mint = Pubkey::new_unique();
        let quote_mint = Pubkey::new_unique();
        let (oracle_pda, init_slot) =
            init_oracle(&mut svm, &payer, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        do_update_price(&mut svm, &payer, &oracle_pda, 100, init_slot + 10);
        do_update_price(&mut svm, &payer, &oracle_pda, 300, init_slot + 20);
        do_update_price(&mut svm, &payer, &oracle_pda, 500, init_slot + 30);

        let oracle = deserialize_oracle(&svm, &oracle_pda);
        let (obs_pda, _) = observation_buffer_pda(&oracle_pda);
        let buffer = deserialize_observation_buffer(&svm, &obs_pda);

        // Compute SWAP between observation at slot+10 and current state at slot+30
        // obs at slot+10: cumulative=0
        // current: cumulative = 0 + 100*10 + 300*10 = 4000, slot=init+30
        let past_obs = get_observation_before_slot(&buffer, init_slot + 15).unwrap();
        assert_eq!(past_obs.slot, init_slot + 10);

        let swap = compute_swap(
            oracle.cumulative_price,
            past_obs.cumulative_price,
            oracle.last_slot,
            past_obs.slot,
        )
        .unwrap();
        // (4000 - 0) / (30 - 10) = 200
        assert_eq!(swap, 200);
    }
}
