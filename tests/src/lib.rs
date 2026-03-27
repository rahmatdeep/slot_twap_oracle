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

    use litesvm::types::{FailedTransactionMetadata, TransactionMetadata};
    use solana_sdk::instruction::InstructionError;
    use solana_sdk::transaction::TransactionError;

    use solana_sdk::program_pack::Pack;

    use anchor_lang::prelude::Discriminator;
    use base64::Engine;

    use slot_twap_oracle::errors::OracleError;
    use slot_twap_oracle::events::OracleUpdate;
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
        let mut svm = LiteSVM::new().with_spl_programs();
        svm.add_program_from_file(program_id(), "../target/deploy/slot_twap_oracle.so")
            .expect("Failed to load program");
        svm
    }

    /// Create a Token-2022 mint account and return its pubkey
    fn create_mint(svm: &mut LiteSVM, payer: &Keypair) -> Pubkey {
        let mint = Keypair::new();
        let rent = svm.minimum_balance_for_rent_exemption(spl_token_2022::state::Mint::LEN);

        let create_account_ix = solana_sdk::system_instruction::create_account(
            &payer.pubkey(),
            &mint.pubkey(),
            rent,
            spl_token_2022::state::Mint::LEN as u64,
            &spl_token_2022::id(),
        );
        let init_mint_ix = spl_token_2022::instruction::initialize_mint2(
            &spl_token_2022::id(),
            &mint.pubkey(),
            &payer.pubkey(),
            None,
            6,
        )
        .unwrap();

        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(
            &[create_account_ix, init_mint_ix],
            Some(&payer.pubkey()),
            &[payer, &mint],
            blockhash,
        );
        svm.send_transaction(tx).expect("Failed to create mint");
        mint.pubkey()
    }

    fn build_initialize_ix(
        authority: &Pubkey,
        base_mint: &Pubkey,
        quote_mint: &Pubkey,
        capacity: u32,
    ) -> Instruction {
        let (oracle_pda, _) = oracle_pda(base_mint, quote_mint);
        let (obs_pda, _) = observation_buffer_pda(&oracle_pda);

        let data = slot_twap_oracle::instruction::InitializeOracle {
            capacity,
        }
        .data();

        Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(oracle_pda, false),
                AccountMeta::new(obs_pda, false),
                AccountMeta::new_readonly(*base_mint, false),
                AccountMeta::new_readonly(*quote_mint, false),
                AccountMeta::new(*authority, true),
                AccountMeta::new_readonly(system_program::id(), false),
            ],
            data,
        }
    }

    fn build_get_swap_ix(oracle: &Pubkey, window_slots: u64, max_staleness_slots: u64) -> Instruction {
        let (obs_pda, _) = observation_buffer_pda(oracle);
        let data = slot_twap_oracle::instruction::GetSwap { window_slots, max_staleness_slots }.data();

        Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new_readonly(*oracle, false),
                AccountMeta::new_readonly(obs_pda, false),
            ],
            data,
        }
    }

    fn build_update_price_ix(
        payer: &Pubkey,
        oracle: &Pubkey,
        new_price: u128,
    ) -> Instruction {
        let (obs_pda, _) = observation_buffer_pda(oracle);
        let data = slot_twap_oracle::instruction::UpdatePrice { new_price }.data();

        Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new_readonly(*payer, true),
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

    /// Parse Anchor return value from transaction return data.
    fn parse_return_value<T: AnchorDeserialize>(meta: &TransactionMetadata) -> T {
        let data = &meta.return_data.data;
        T::deserialize(&mut &data[..]).expect("Failed to deserialize return value")
    }

    /// Helper: assert that a failed transaction contains a specific anchor error code
    fn assert_anchor_error(result: &Result<TransactionMetadata, FailedTransactionMetadata>, expected: OracleError) {
        let failed = result.as_ref().expect_err("Expected transaction to fail");
        let expected_code = 6000u32 + expected as u32;
        match &failed.err {
            TransactionError::InstructionError(_, InstructionError::Custom(code)) => {
                assert_eq!(*code, expected_code, "Expected error code {expected_code} ({expected:?}), got {code}");
            }
            other => panic!("Expected InstructionError::Custom, got {other:?}"),
        }
    }

    /// Helper: send get_swap and return the u128 result
    /// Default max staleness used by most tests — large enough to never trigger.
    const DEFAULT_MAX_STALENESS: u64 = 1_000_000;

    fn do_get_swap(
        svm: &mut LiteSVM,
        payer: &Keypair,
        oracle_pda: &Pubkey,
        window_slots: u64,
    ) -> u128 {
        let ix = build_get_swap_ix(oracle_pda, window_slots, DEFAULT_MAX_STALENESS);
        let blockhash = svm.latest_blockhash();
        let tx =
            Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[payer], blockhash);
        let meta = svm.send_transaction(tx).expect("get_swap failed");
        parse_return_value::<u128>(&meta)
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
        authority: &Keypair,
        oracle_pda: &Pubkey,
        new_price: u128,
        target_slot: u64,
    ) {
        svm.warp_to_slot(target_slot);
        svm.expire_blockhash();
        let ix = build_update_price_ix(&authority.pubkey(), oracle_pda, new_price);
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&authority.pubkey()),
            &[authority],
            blockhash,
        );
        svm.send_transaction(tx).unwrap();
    }

    // ── Happy-path tests ──

    #[test]
    fn test_initialize_oracle() {
        let mut svm = setup();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &payer);
        let quote_mint = create_mint(&mut svm, &payer);
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

        let base_mint = create_mint(&mut svm, &payer);
        let quote_mint = create_mint(&mut svm, &payer);
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

        let base_mint = create_mint(&mut svm, &payer);
        let quote_mint = create_mint(&mut svm, &payer);
        let (oracle_pda, init_slot) =
            init_oracle(&mut svm, &payer, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        // First update: set price to 1000, after 10 slots
        do_update_price(&mut svm, &payer, &oracle_pda, 1000, init_slot + 10);
        let oracle = deserialize_oracle(&svm, &oracle_pda);
        assert_eq!(oracle.cumulative_price, 0);
        assert_eq!(oracle.last_price, 1000);

        // Second update: set price to 1100 (+10%), after 20 more slots
        do_update_price(&mut svm, &payer, &oracle_pda, 1100, init_slot + 30);
        let oracle = deserialize_oracle(&svm, &oracle_pda);
        assert_eq!(oracle.cumulative_price, 20_000); // 1000 * 20
        assert_eq!(oracle.last_price, 1100);

        // Third update: set price to 1050 (-4.5%), after 5 more slots
        do_update_price(&mut svm, &payer, &oracle_pda, 1050, init_slot + 35);
        let oracle = deserialize_oracle(&svm, &oracle_pda);
        assert_eq!(oracle.cumulative_price, 25_500); // 20000 + 1100*5
        assert_eq!(oracle.last_price, 1050);
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

        let base_mint = create_mint(&mut svm, &payer);
        let quote_mint = create_mint(&mut svm, &payer);
        let (oracle_pda, init_slot) =
            init_oracle(&mut svm, &payer, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        do_update_price(&mut svm, &payer, &oracle_pda, 1000, init_slot + 5);
        do_update_price(&mut svm, &payer, &oracle_pda, 1100, init_slot + 15);
        do_update_price(&mut svm, &payer, &oracle_pda, 1050, init_slot + 25);

        let (obs_pda, _) = observation_buffer_pda(&oracle_pda);
        let buffer = deserialize_observation_buffer(&svm, &obs_pda);

        assert_eq!(buffer.observations.len(), 3);
        assert_eq!(buffer.observations[0].slot, init_slot + 5);
        assert_eq!(buffer.observations[0].cumulative_price, 0); // 0 * 5
        assert_eq!(buffer.observations[1].slot, init_slot + 15);
        assert_eq!(buffer.observations[1].cumulative_price, 10_000); // 0 + 1000*10
        assert_eq!(buffer.observations[2].slot, init_slot + 25);
        assert_eq!(buffer.observations[2].cumulative_price, 21_000); // 10_000 + 1100*10
        assert_eq!(buffer.head, 3);
    }

    #[test]
    fn test_observation_buffer_ring_wraps() {
        let mut svm = setup();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &payer);
        let quote_mint = create_mint(&mut svm, &payer);
        let capacity = 3u32;
        let (oracle_pda, init_slot) =
            init_oracle(&mut svm, &payer, &base_mint, &quote_mint, capacity);

        // Fill the buffer (3 updates = capacity)
        do_update_price(&mut svm, &payer, &oracle_pda, 1000, init_slot + 10);
        do_update_price(&mut svm, &payer, &oracle_pda, 1100, init_slot + 20);
        do_update_price(&mut svm, &payer, &oracle_pda, 1050, init_slot + 30);

        let (obs_pda, _) = observation_buffer_pda(&oracle_pda);
        let buffer = deserialize_observation_buffer(&svm, &obs_pda);
        assert_eq!(buffer.observations.len(), 3);
        assert_eq!(buffer.head, 0); // wrapped around

        // 4th update should overwrite index 0
        do_update_price(&mut svm, &payer, &oracle_pda, 1090, init_slot + 40);

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

        let base_mint = create_mint(&mut svm, &payer);
        let quote_mint = create_mint(&mut svm, &payer);
        let (oracle_pda, init_slot) =
            init_oracle(&mut svm, &payer, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        do_update_price(&mut svm, &payer, &oracle_pda, 1000, init_slot + 10);
        do_update_price(&mut svm, &payer, &oracle_pda, 1100, init_slot + 20);
        do_update_price(&mut svm, &payer, &oracle_pda, 1050, init_slot + 30);

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

        let base_mint = create_mint(&mut svm, &payer);
        let quote_mint = create_mint(&mut svm, &payer);
        let (oracle_pda, init_slot) =
            init_oracle(&mut svm, &payer, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        let snap_slot_past = init_slot;
        let snap_cumulative_past = 0u128;

        do_update_price(&mut svm, &payer, &oracle_pda, 1000, init_slot + 10);
        do_update_price(&mut svm, &payer, &oracle_pda, 1100, init_slot + 20);
        do_update_price(&mut svm, &payer, &oracle_pda, 1050, init_slot + 40);

        let oracle = deserialize_oracle(&svm, &oracle_pda);
        // cumul = 0 + 1000*10 + 1100*20 = 32000
        assert_eq!(oracle.cumulative_price, 32_000);

        let swap = compute_swap(
            oracle.cumulative_price,
            snap_cumulative_past,
            oracle.last_slot,
            snap_slot_past,
        )
        .unwrap();
        // 32000 / (init_slot+40 - init_slot) = 32000/40 = 800
        assert_eq!(swap, 800);
    }

    #[test]
    fn test_compute_swap_from_observation_buffer() {
        let mut svm = setup();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &payer);
        let quote_mint = create_mint(&mut svm, &payer);
        let (oracle_pda, init_slot) =
            init_oracle(&mut svm, &payer, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        do_update_price(&mut svm, &payer, &oracle_pda, 1000, init_slot + 10);
        do_update_price(&mut svm, &payer, &oracle_pda, 1100, init_slot + 20);
        do_update_price(&mut svm, &payer, &oracle_pda, 1050, init_slot + 30);

        let oracle = deserialize_oracle(&svm, &oracle_pda);
        let (obs_pda, _) = observation_buffer_pda(&oracle_pda);
        let buffer = deserialize_observation_buffer(&svm, &obs_pda);

        // Compute SWAP between observation at slot+10 and current state at slot+30
        // obs at slot+10: cumulative=0
        // current: cumulative = 0 + 1000*10 + 1100*10 = 21000, slot=init+30
        let past_obs = get_observation_before_slot(&buffer, init_slot + 15).unwrap();
        assert_eq!(past_obs.slot, init_slot + 10);

        let swap = compute_swap(
            oracle.cumulative_price,
            past_obs.cumulative_price,
            oracle.last_slot,
            past_obs.slot,
        )
        .unwrap();
        // (21000 - 0) / (30 - 10) = 1050
        assert_eq!(swap, 1050);
    }

    // ── get_swap instruction tests ──

    #[test]
    fn test_get_swap_basic() {
        let mut svm = setup();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &payer);
        let quote_mint = create_mint(&mut svm, &payer);
        let (oracle_pda, init_slot) =
            init_oracle(&mut svm, &payer, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        // Price=1000 for 20 slots, then price=1100 for 10 slots
        do_update_price(&mut svm, &payer, &oracle_pda, 1000, init_slot + 10);
        do_update_price(&mut svm, &payer, &oracle_pda, 1100, init_slot + 30);

        // Warp to slot+40 so there's elapsed time since last update
        svm.warp_to_slot(init_slot + 40);
        svm.expire_blockhash();

        // At slot init_slot+40:
        // on-chain cumul = 0 + 1000*20 = 20_000
        // cumulative_now = 20_000 + 1100*(40-30) = 31_000
        // window_slots=30 → window_start = (init_slot+40) - 30 = init_slot+10
        // Past obs: need slot <= init_slot+10 → observation at init_slot+10, cumulative=0
        // SWAP = (31_000 - 0) / (init_slot+40 - init_slot-10) = 31_000/30 = 1033
        let swap = do_get_swap(&mut svm, &payer, &oracle_pda, 30);
        assert_eq!(swap, 1033);
    }

    #[test]
    fn test_get_swap_constant_price() {
        let mut svm = setup();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &payer);
        let quote_mint = create_mint(&mut svm, &payer);
        let (oracle_pda, init_slot) =
            init_oracle(&mut svm, &payer, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        // Set constant price of 1000
        do_update_price(&mut svm, &payer, &oracle_pda, 1000, init_slot + 10);
        do_update_price(&mut svm, &payer, &oracle_pda, 1000, init_slot + 20);
        do_update_price(&mut svm, &payer, &oracle_pda, 1000, init_slot + 30);

        svm.warp_to_slot(init_slot + 40);
        svm.expire_blockhash();

        // cumulative_now = 0 + 1000*10 + 1000*10 + 1000*10 = 30_000
        // window=30: window_start = init_slot+10, past_obs at slot init_slot+10 (cumul=0)
        // SWAP = 30_000 / 30 = 1000
        let swap = do_get_swap(&mut svm, &payer, &oracle_pda, 30);
        assert_eq!(swap, 1000);
    }

    #[test]
    fn test_get_swap_insufficient_observations() {
        let mut svm = setup();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &payer);
        let quote_mint = create_mint(&mut svm, &payer);
        let (oracle_pda, _init_slot) =
            init_oracle(&mut svm, &payer, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        // No updates — buffer is empty, get_swap should fail
        let ix = build_get_swap_ix(&oracle_pda, 10, DEFAULT_MAX_STALENESS);
        let blockhash = svm.latest_blockhash();
        let tx =
            Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[&payer], blockhash);
        let result = svm.send_transaction(tx);
        assert!(result.is_err());
    }

    #[test]
    fn test_get_swap_window_too_large() {
        let mut svm = setup();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &payer);
        let quote_mint = create_mint(&mut svm, &payer);
        let (oracle_pda, init_slot) =
            init_oracle(&mut svm, &payer, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        do_update_price(&mut svm, &payer, &oracle_pda, 500, init_slot + 10);

        svm.warp_to_slot(init_slot + 20);
        svm.expire_blockhash();

        // Window of 1000 slots is larger than any observation history
        // window_start = (init_slot+20) - 1000 — underflows, returning InsufficientHistory
        let ix = build_get_swap_ix(&oracle_pda, 1000, DEFAULT_MAX_STALENESS);
        let blockhash = svm.latest_blockhash();
        let tx =
            Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[&payer], blockhash);
        let result = svm.send_transaction(tx);
        assert_anchor_error(&result, OracleError::InsufficientHistory);
    }

    #[test]
    fn test_get_swap_no_observation_before_window() {
        let mut svm = setup();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &payer);
        let quote_mint = create_mint(&mut svm, &payer);
        let (oracle_pda, init_slot) =
            init_oracle(&mut svm, &payer, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        // Single observation at init_slot+10
        do_update_price(&mut svm, &payer, &oracle_pda, 500, init_slot + 10);

        svm.warp_to_slot(init_slot + 20);
        svm.expire_blockhash();

        // Window of 15 slots: window_start = (init_slot+20) - 15 = init_slot+5
        // No observation exists before init_slot+5+1 = init_slot+6, so InsufficientHistory
        let ix = build_get_swap_ix(&oracle_pda, 15, DEFAULT_MAX_STALENESS);
        let blockhash = svm.latest_blockhash();
        let tx =
            Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[&payer], blockhash);
        let result = svm.send_transaction(tx);
        assert_anchor_error(&result, OracleError::InsufficientHistory);
    }

    // ── Scenario tests (mocked slots) ──

    #[test]
    fn test_initialize_oracle_creates_account_correctly() {
        let mut svm = setup();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &payer);
        let quote_mint = create_mint(&mut svm, &payer);

        // Warp to a known slot so init is deterministic
        svm.warp_to_slot(50);
        svm.expire_blockhash();

        let (oracle_pda, _) = oracle_pda(&base_mint, &quote_mint);
        let ix = build_initialize_ix(&payer.pubkey(), &base_mint, &quote_mint, DEFAULT_CAPACITY);
        let blockhash = svm.latest_blockhash();
        let tx =
            Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[&payer], blockhash);
        svm.send_transaction(tx).unwrap();

        let oracle = deserialize_oracle(&svm, &oracle_pda);
        assert_eq!(oracle.base_mint, base_mint);
        assert_eq!(oracle.quote_mint, quote_mint);
        assert_eq!(oracle.last_price, 0);
        assert_eq!(oracle.cumulative_price, 0);
        assert_eq!(oracle.last_slot, 50);

        let (obs_pda, _) = observation_buffer_pda(&oracle_pda);
        let buffer = deserialize_observation_buffer(&svm, &obs_pda);
        assert_eq!(buffer.oracle, oracle_pda);
        assert_eq!(buffer.head, 0);
        assert_eq!(buffer.capacity, DEFAULT_CAPACITY);
        assert!(buffer.observations.is_empty());
    }

    #[test]
    fn test_update_price_cumulative_math_three_updates() {
        // Simulate: slot 100 price 1000, slot 110 price 1100, slot 120 price 1050
        let mut svm = setup();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &payer);
        let quote_mint = create_mint(&mut svm, &payer);

        // Initialize at slot 90 so last_slot = 90
        svm.warp_to_slot(90);
        svm.expire_blockhash();
        let (oracle_pda, _) = oracle_pda(&base_mint, &quote_mint);
        let ix = build_initialize_ix(&payer.pubkey(), &base_mint, &quote_mint, DEFAULT_CAPACITY);
        let blockhash = svm.latest_blockhash();
        let tx =
            Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[&payer], blockhash);
        svm.send_transaction(tx).unwrap();

        let oracle = deserialize_oracle(&svm, &oracle_pda);
        assert_eq!(oracle.last_slot, 90);

        // Slot 100: set price=1000
        // slot_delta = 100 - 90 = 10, weighted = 0 * 10 = 0
        // cumulative = 0 + 0 = 0, last_price = 1000, last_slot = 100
        do_update_price(&mut svm, &payer, &oracle_pda, 1000, 100);
        let oracle = deserialize_oracle(&svm, &oracle_pda);
        assert_eq!(oracle.last_price, 1000);
        assert_eq!(oracle.cumulative_price, 0);
        assert_eq!(oracle.last_slot, 100);

        // Slot 110: set price=1100
        // slot_delta = 110 - 100 = 10, weighted = 1000 * 10 = 10000
        // cumulative = 0 + 10000 = 10000, last_price = 1100, last_slot = 110
        do_update_price(&mut svm, &payer, &oracle_pda, 1100, 110);
        let oracle = deserialize_oracle(&svm, &oracle_pda);
        assert_eq!(oracle.last_price, 1100);
        assert_eq!(oracle.cumulative_price, 10_000);
        assert_eq!(oracle.last_slot, 110);

        // Slot 120: set price=1050
        // slot_delta = 120 - 110 = 10, weighted = 1100 * 10 = 11000
        // cumulative = 10000 + 11000 = 21000, last_price = 1050, last_slot = 120
        do_update_price(&mut svm, &payer, &oracle_pda, 1050, 120);
        let oracle = deserialize_oracle(&svm, &oracle_pda);
        assert_eq!(oracle.last_price, 1050);
        assert_eq!(oracle.cumulative_price, 21_000);
        assert_eq!(oracle.last_slot, 120);
    }

    #[test]
    fn test_get_swap_over_20_slot_window() {
        // After the three updates above, test SWAP over a 20-slot window
        let mut svm = setup();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &payer);
        let quote_mint = create_mint(&mut svm, &payer);

        // Initialize at slot 90
        svm.warp_to_slot(90);
        svm.expire_blockhash();
        let (oracle_pda, _) = oracle_pda(&base_mint, &quote_mint);
        let ix = build_initialize_ix(&payer.pubkey(), &base_mint, &quote_mint, DEFAULT_CAPACITY);
        let blockhash = svm.latest_blockhash();
        let tx =
            Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[&payer], blockhash);
        svm.send_transaction(tx).unwrap();

        // slot 100: price=1000, cumulative=0
        do_update_price(&mut svm, &payer, &oracle_pda, 1000, 100);
        // slot 110: price=1100, cumulative=10000
        do_update_price(&mut svm, &payer, &oracle_pda, 1100, 110);
        // slot 120: price=1050, cumulative=21000
        do_update_price(&mut svm, &payer, &oracle_pda, 1050, 120);

        // Call get_swap at slot 120 with window_slots=20
        // current_slot = 120, slot_delta_since_last = 0
        // cumulative_now = 21000
        // window_start = 120 - 20 = 100
        // Past obs: need slot <= 100 → observation at slot 100 (cumulative=0)
        // SWAP = (21000 - 0) / (120 - 100) = 21000 / 20 = 1050
        let swap = do_get_swap(&mut svm, &payer, &oracle_pda, 20);
        assert_eq!(swap, 1050);

        // Verify: this makes sense because over slots 100-120:
        // price=1000 for 10 slots (100-110), price=1100 for 10 slots (110-120)
        // weighted avg = (1000*10 + 1100*10) / 20 = 21000/20 = 1050
    }

    // ── Edge case tests ──

    /// Helper: send a tx and expect it to fail
    fn send_tx_expect_err(
        svm: &mut LiteSVM,
        payer: &Keypair,
        instructions: &[Instruction],
    ) {
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(
            instructions,
            Some(&payer.pubkey()),
            &[payer],
            blockhash,
        );
        let result = svm.send_transaction(tx);
        assert!(result.is_err(), "Expected transaction to fail");
    }

    #[test]
    fn test_double_initialize_same_pair_fails() {
        let mut svm = setup();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &payer);
        let quote_mint = create_mint(&mut svm, &payer);
        init_oracle(&mut svm, &payer, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        // Second init with same mints should fail (PDA already exists)
        let ix = build_initialize_ix(&payer.pubkey(), &base_mint, &quote_mint, DEFAULT_CAPACITY);
        send_tx_expect_err(&mut svm, &payer, &[ix]);
    }

    #[test]
    fn test_initialize_zero_capacity_fails() {
        let mut svm = setup();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &payer);
        let quote_mint = create_mint(&mut svm, &payer);

        let ix = build_initialize_ix(&payer.pubkey(), &base_mint, &quote_mint, 0);
        send_tx_expect_err(&mut svm, &payer, &[ix]);
    }

    #[test]
    fn test_different_pairs_are_independent() {
        let mut svm = setup();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();

        let mint_a = create_mint(&mut svm, &payer);
        let mint_b = create_mint(&mut svm, &payer);
        let mint_c = create_mint(&mut svm, &payer);

        let (oracle_ab, init_slot_ab) =
            init_oracle(&mut svm, &payer, &mint_a, &mint_b, DEFAULT_CAPACITY);
        let (oracle_ac, init_slot_ac) =
            init_oracle(&mut svm, &payer, &mint_a, &mint_c, DEFAULT_CAPACITY);

        do_update_price(&mut svm, &payer, &oracle_ab, 100, init_slot_ab + 10);
        do_update_price(&mut svm, &payer, &oracle_ac, 999, init_slot_ac + 10);

        let ab = deserialize_oracle(&svm, &oracle_ab);
        let ac = deserialize_oracle(&svm, &oracle_ac);
        assert_eq!(ab.last_price, 100);
        assert_eq!(ac.last_price, 999);
    }

    #[test]
    fn test_update_price_stale_slot_fails() {
        let mut svm = setup();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &payer);
        let quote_mint = create_mint(&mut svm, &payer);
        let (oracle_pda, init_slot) =
            init_oracle(&mut svm, &payer, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        do_update_price(&mut svm, &payer, &oracle_pda, 1000, init_slot + 10);

        // Try updating at the same slot (no warp) — should fail with StaleSlot
        let ix = build_update_price_ix(&payer.pubkey(), &oracle_pda, 1100);
        send_tx_expect_err(&mut svm, &payer, &[ix]);
    }

    #[test]
    fn test_update_price_permissionless() {
        // Any signer can update the oracle — not just the initializer.
        let mut svm = setup();
        let initializer = Keypair::new();
        let updater_a = Keypair::new();
        let updater_b = Keypair::new();
        svm.airdrop(&initializer.pubkey(), 10_000_000_000).unwrap();
        svm.airdrop(&updater_a.pubkey(), 10_000_000_000).unwrap();
        svm.airdrop(&updater_b.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &initializer);
        let quote_mint = create_mint(&mut svm, &initializer);
        let (oracle_pda, init_slot) =
            init_oracle(&mut svm, &initializer, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        // updater_a (different from initializer) updates successfully
        do_update_price(&mut svm, &updater_a, &oracle_pda, 1000, init_slot + 10);
        let oracle = deserialize_oracle(&svm, &oracle_pda);
        assert_eq!(oracle.last_price, 1000);

        // updater_b (yet another signer) also updates successfully
        do_update_price(&mut svm, &updater_b, &oracle_pda, 1100, init_slot + 20);
        let oracle = deserialize_oracle(&svm, &oracle_pda);
        assert_eq!(oracle.last_price, 1100);
        // cumulative = 0 + 1000*10 = 10000
        assert_eq!(oracle.cumulative_price, 10_000);

        // initializer can still update too
        do_update_price(&mut svm, &initializer, &oracle_pda, 1050, init_slot + 30);
        let oracle = deserialize_oracle(&svm, &oracle_pda);
        assert_eq!(oracle.last_price, 1050);
        // cumulative = 10000 + 1100*10 = 21000
        assert_eq!(oracle.cumulative_price, 21_000);
    }

    #[test]
    fn test_update_price_zero_price() {
        let mut svm = setup();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &payer);
        let quote_mint = create_mint(&mut svm, &payer);
        let (oracle_pda, init_slot) =
            init_oracle(&mut svm, &payer, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        // Set price to 1000, then decrease within bounds, then increase
        do_update_price(&mut svm, &payer, &oracle_pda, 1000, init_slot + 10);
        do_update_price(&mut svm, &payer, &oracle_pda, 950, init_slot + 20);

        let oracle = deserialize_oracle(&svm, &oracle_pda);
        assert_eq!(oracle.last_price, 950);
        // cumulative = 0 + 1000*10 = 10000
        assert_eq!(oracle.cumulative_price, 10_000);

        // Another update — price goes back up within bounds
        do_update_price(&mut svm, &payer, &oracle_pda, 1000, init_slot + 30);
        let oracle = deserialize_oracle(&svm, &oracle_pda);
        // cumulative = 10000 + 950*10 = 19500
        assert_eq!(oracle.cumulative_price, 19_500);
    }

    #[test]
    fn test_update_price_large_values() {
        let mut svm = setup();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &payer);
        let quote_mint = create_mint(&mut svm, &payer);
        let (oracle_pda, init_slot) =
            init_oracle(&mut svm, &payer, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        // Use a large price that fits in u128 but tests big arithmetic
        let big_price: u128 = 1_000_000_000_000_000_000; // 1e18
        do_update_price(&mut svm, &payer, &oracle_pda, big_price, init_slot + 10);

        let oracle = deserialize_oracle(&svm, &oracle_pda);
        assert_eq!(oracle.last_price, big_price);
        assert_eq!(oracle.cumulative_price, 0); // first update from price=0

        do_update_price(&mut svm, &payer, &oracle_pda, big_price, init_slot + 20);
        let oracle = deserialize_oracle(&svm, &oracle_pda);
        // cumulative = 1e18 * 10 = 1e19
        assert_eq!(oracle.cumulative_price, big_price * 10);
    }

    #[test]
    fn test_update_price_single_slot_delta() {
        // Minimum valid slot delta is 1
        let mut svm = setup();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &payer);
        let quote_mint = create_mint(&mut svm, &payer);
        let (oracle_pda, init_slot) =
            init_oracle(&mut svm, &payer, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        do_update_price(&mut svm, &payer, &oracle_pda, 1000, init_slot + 1);
        let oracle = deserialize_oracle(&svm, &oracle_pda);
        assert_eq!(oracle.last_slot, init_slot + 1);
        assert_eq!(oracle.cumulative_price, 0); // 0 * 1

        do_update_price(&mut svm, &payer, &oracle_pda, 1100, init_slot + 2);
        let oracle = deserialize_oracle(&svm, &oracle_pda);
        // cumulative = 0 + 1000 * 1 = 1000
        assert_eq!(oracle.cumulative_price, 1000);
    }

    #[test]
    fn test_observation_buffer_capacity_one() {
        let mut svm = setup();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &payer);
        let quote_mint = create_mint(&mut svm, &payer);
        let (oracle_pda, init_slot) =
            init_oracle(&mut svm, &payer, &base_mint, &quote_mint, 1);

        do_update_price(&mut svm, &payer, &oracle_pda, 1000, init_slot + 10);

        let (obs_pda, _) = observation_buffer_pda(&oracle_pda);
        let buffer = deserialize_observation_buffer(&svm, &obs_pda);
        assert_eq!(buffer.observations.len(), 1);
        assert_eq!(buffer.observations[0].slot, init_slot + 10);

        // Second update overwrites the only slot
        do_update_price(&mut svm, &payer, &oracle_pda, 1100, init_slot + 20);
        let buffer = deserialize_observation_buffer(&svm, &obs_pda);
        assert_eq!(buffer.observations.len(), 1);
        assert_eq!(buffer.observations[0].slot, init_slot + 20);
        assert_eq!(buffer.observations[0].cumulative_price, 10_000); // 1000 * 10
    }

    #[test]
    fn test_get_observation_before_slot_after_wrap() {
        // After ring wraps, ensure lookup still finds correct entries
        let mut svm = setup();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &payer);
        let quote_mint = create_mint(&mut svm, &payer);
        let (oracle_pda, init_slot) =
            init_oracle(&mut svm, &payer, &base_mint, &quote_mint, 3);

        // Fill buffer: slots +10, +20, +30
        do_update_price(&mut svm, &payer, &oracle_pda, 1000, init_slot + 10);
        do_update_price(&mut svm, &payer, &oracle_pda, 1100, init_slot + 20);
        do_update_price(&mut svm, &payer, &oracle_pda, 1050, init_slot + 30);

        // Overwrite oldest: slot +40 replaces slot +10
        do_update_price(&mut svm, &payer, &oracle_pda, 1090, init_slot + 40);

        let (obs_pda, _) = observation_buffer_pda(&oracle_pda);
        let buffer = deserialize_observation_buffer(&svm, &obs_pda);

        // Slot +10 is gone. Looking before +25 should find +20
        let obs = get_observation_before_slot(&buffer, init_slot + 25).unwrap();
        assert_eq!(obs.slot, init_slot + 20);

        // Looking before +35 should find +30
        let obs = get_observation_before_slot(&buffer, init_slot + 35).unwrap();
        assert_eq!(obs.slot, init_slot + 30);

        // Looking before +41 should find +40 (the newest)
        let obs = get_observation_before_slot(&buffer, init_slot + 41).unwrap();
        assert_eq!(obs.slot, init_slot + 40);

        // Looking before +20 — slot +10 is overwritten, nothing qualifies
        let obs = get_observation_before_slot(&buffer, init_slot + 20);
        assert!(obs.is_none());
    }

    #[test]
    fn test_get_swap_with_elapsed_time_since_last_update() {
        // get_swap should extend cumulative to current slot even without an update
        let mut svm = setup();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &payer);
        let quote_mint = create_mint(&mut svm, &payer);
        let (oracle_pda, init_slot) =
            init_oracle(&mut svm, &payer, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        // Set price=100 at slot+10
        do_update_price(&mut svm, &payer, &oracle_pda, 100, init_slot + 10);

        // Warp to slot+110 without any more updates
        svm.warp_to_slot(init_slot + 110);
        svm.expire_blockhash();

        // cumulative_now = 0 + 100*(110 - 10) = 10_000
        // window=100: window_start = 110-100 = init_slot+10
        // past_obs at slot init_slot+10 (cumul=0)
        // SWAP = 10_000 / (110 - 10) = 100
        let swap = do_get_swap(&mut svm, &payer, &oracle_pda, 100);
        assert_eq!(swap, 100);
    }

    #[test]
    fn test_get_swap_single_observation() {
        // Only one observation exists — get_swap should work if window covers it
        let mut svm = setup();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &payer);
        let quote_mint = create_mint(&mut svm, &payer);
        let (oracle_pda, init_slot) =
            init_oracle(&mut svm, &payer, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        do_update_price(&mut svm, &payer, &oracle_pda, 250, init_slot + 10);

        svm.warp_to_slot(init_slot + 30);
        svm.expire_blockhash();

        // cumulative_now = 0 + 250*(30-10) = 5000
        // window=20: window_start = 30-20 = init_slot+10
        // past_obs: slot init_slot+10 (cumul=0)
        // SWAP = 5000 / 20 = 250
        let swap = do_get_swap(&mut svm, &payer, &oracle_pda, 20);
        assert_eq!(swap, 250);
    }

    #[test]
    fn test_get_swap_window_exactly_on_observation() {
        // Window start lands exactly on an observation slot
        let mut svm = setup();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &payer);
        let quote_mint = create_mint(&mut svm, &payer);
        let (oracle_pda, init_slot) =
            init_oracle(&mut svm, &payer, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        do_update_price(&mut svm, &payer, &oracle_pda, 1000, init_slot + 10);
        do_update_price(&mut svm, &payer, &oracle_pda, 1100, init_slot + 20);

        svm.warp_to_slot(init_slot + 30);
        svm.expire_blockhash();

        // cumulative_now = 0 + 1000*10 + 1100*10 = 21000
        // window=20: window_start = 30-20 = init_slot+10
        // get_observation_before_slot(init_slot+11) → slot init_slot+10 (cumul=0)
        // SWAP = 21000 / (30-10) = 1050
        let swap = do_get_swap(&mut svm, &payer, &oracle_pda, 20);
        assert_eq!(swap, 1050);
    }

    #[test]
    fn test_get_swap_after_ring_buffer_wraps() {
        // Ensure get_swap still works after oldest observations are overwritten
        let mut svm = setup();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &payer);
        let quote_mint = create_mint(&mut svm, &payer);
        let (oracle_pda, init_slot) =
            init_oracle(&mut svm, &payer, &base_mint, &quote_mint, 3);

        // Fill buffer: 3 observations
        do_update_price(&mut svm, &payer, &oracle_pda, 1000, init_slot + 10);
        do_update_price(&mut svm, &payer, &oracle_pda, 1100, init_slot + 20);
        do_update_price(&mut svm, &payer, &oracle_pda, 1050, init_slot + 30);

        // Wrap: overwrites slot+10
        do_update_price(&mut svm, &payer, &oracle_pda, 1090, init_slot + 40);

        svm.warp_to_slot(init_slot + 50);
        svm.expire_blockhash();

        // cumulative at slot+10: 0
        // cumulative at slot+20: 0 + 1000*10 = 10000
        // cumulative at slot+30: 10000 + 1100*10 = 21000
        // cumulative at slot+40: 21000 + 1050*10 = 31500
        // cumulative_now = 31500 + 1090*10 = 42400
        // window=20: window_start = init_slot+30
        // past_obs: need slot <= init_slot+30 → slot+30 (cumul=21000)
        // SWAP = (42400 - 21000) / (50 - 30) = 21400/20 = 1070
        let swap = do_get_swap(&mut svm, &payer, &oracle_pda, 20);
        assert_eq!(swap, 1070);
    }

    #[test]
    fn test_compute_swap_zero_slot_delta_fails() {
        let result = compute_swap(1000, 0, 10, 10);
        assert!(result.is_err());
    }

    #[test]
    fn test_many_rapid_updates() {
        // Stress test: many updates, 1 slot apart each
        let mut svm = setup();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &payer);
        let quote_mint = create_mint(&mut svm, &payer);
        let (oracle_pda, init_slot) =
            init_oracle(&mut svm, &payer, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        // 20 updates, alternating price between 1000 and 1100
        for i in 1..=20u64 {
            let price = if i % 2 == 0 { 1100 } else { 1000 };
            do_update_price(&mut svm, &payer, &oracle_pda, price, init_slot + i);
        }

        let oracle = deserialize_oracle(&svm, &oracle_pda);
        assert_eq!(oracle.last_slot, init_slot + 20);
        assert_eq!(oracle.last_price, 1100);

        // cumulative: first update (i=1) adds 0*1=0, second (i=2) adds 1000*1=1000,
        // third (i=3) adds 1100*1=1100, etc.
        // Weighted contributions from previous price:
        // slot 1: prev=0, delta=1 → 0
        // slot 2: prev=1000, delta=1 → 1000
        // slot 3: prev=1100, delta=1 → 1100
        // ...pattern: 0, then alternating 1000, 1100
        // Slots 2-20 (19 values): 1000,1100 alternating starting with 1000
        // 10 values of 1000, 9 values of 1100 = 10000 + 9900 = 19900
        assert_eq!(oracle.cumulative_price, 19_900);
    }

    // ── Sealevel parallel execution tests ──
    //
    // Solana's Sealevel runtime can execute transactions in parallel when their
    // account sets don't overlap. Each oracle pair (SOL/USDC, ETH/USDC, BTC/USDC)
    // has its own Oracle PDA and ObservationBuffer PDA derived from unique mint
    // pairs. Because update_price only writes to the pair's own two accounts,
    // the runtime sees no read/write conflicts and can schedule all three
    // transactions in the same slot concurrently.
    //
    // In this test we demonstrate this by:
    // 1. Batching all three update_price instructions into a single transaction
    //    (proving no account conflicts within a tx)
    // 2. Sending three separate transactions in sequence within the same slot
    //    (proving no cross-tx contention — on a real validator these would be
    //    parallelized by the scheduler)

    #[test]
    fn test_parallel_updates_single_tx_three_pairs() {
        // Three update_price instructions for different pairs in ONE transaction.
        // This only works if their writable account sets are disjoint — exactly
        // the property that enables Sealevel parallelism.

        let mut svm = setup();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();

        // Mint stand-ins for SOL, ETH, BTC, USDC
        let sol_mint = create_mint(&mut svm, &payer);
        let eth_mint = create_mint(&mut svm, &payer);
        let btc_mint = create_mint(&mut svm, &payer);
        let usdc_mint = create_mint(&mut svm, &payer);

        // Initialize all three pairs
        let (sol_oracle, _) =
            init_oracle(&mut svm, &payer, &sol_mint, &usdc_mint, DEFAULT_CAPACITY);
        let (eth_oracle, _) =
            init_oracle(&mut svm, &payer, &eth_mint, &usdc_mint, DEFAULT_CAPACITY);
        let (btc_oracle, init_slot) =
            init_oracle(&mut svm, &payer, &btc_mint, &usdc_mint, DEFAULT_CAPACITY);

        // Warp forward so slot_delta > 0
        svm.warp_to_slot(init_slot + 10);
        svm.expire_blockhash();

        // Build three update_price instructions — each touches only its own
        // oracle + observation_buffer accounts. No overlap.
        let ix_sol = build_update_price_ix(&payer.pubkey(), &sol_oracle, 100);
        let ix_eth = build_update_price_ix(&payer.pubkey(), &eth_oracle, 200);
        let ix_btc = build_update_price_ix(&payer.pubkey(), &btc_oracle, 300);

        // Send all three in a SINGLE transaction. This succeeds because Solana
        // allows multiple instructions in one tx as long as there are no
        // conflicting writable account locks. Each pair's accounts are unique
        // PDAs, so there is zero contention.
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(
            &[ix_sol, ix_eth, ix_btc],
            Some(&payer.pubkey()),
            &[&payer],
            blockhash,
        );
        svm.send_transaction(tx).expect("Batched update_price failed");

        // Verify each pair updated independently
        let sol = deserialize_oracle(&svm, &sol_oracle);
        assert_eq!(sol.last_price, 100);
        assert_eq!(sol.last_slot, init_slot + 10);

        let eth = deserialize_oracle(&svm, &eth_oracle);
        assert_eq!(eth.last_price, 200);
        assert_eq!(eth.last_slot, init_slot + 10);

        let btc = deserialize_oracle(&svm, &btc_oracle);
        assert_eq!(btc.last_price, 300);
        assert_eq!(btc.last_slot, init_slot + 10);

        // Verify observation buffers are independent
        let (sol_obs, _) = observation_buffer_pda(&sol_oracle);
        let (eth_obs, _) = observation_buffer_pda(&eth_oracle);
        let (btc_obs, _) = observation_buffer_pda(&btc_oracle);

        let sol_buf = deserialize_observation_buffer(&svm, &sol_obs);
        let eth_buf = deserialize_observation_buffer(&svm, &eth_obs);
        let btc_buf = deserialize_observation_buffer(&svm, &btc_obs);

        assert_eq!(sol_buf.observations.len(), 1);
        assert_eq!(eth_buf.observations.len(), 1);
        assert_eq!(btc_buf.observations.len(), 1);
    }

    #[test]
    fn test_parallel_updates_separate_txs_same_slot() {
        // Three separate transactions in the same slot — on a real validator
        // the Sealevel scheduler would run these in parallel because their
        // write-lock sets are disjoint.

        let mut svm = setup();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();

        let sol_mint = create_mint(&mut svm, &payer);
        let eth_mint = create_mint(&mut svm, &payer);
        let btc_mint = create_mint(&mut svm, &payer);
        let usdc_mint = create_mint(&mut svm, &payer);

        let (sol_oracle, _) =
            init_oracle(&mut svm, &payer, &sol_mint, &usdc_mint, DEFAULT_CAPACITY);
        let (eth_oracle, _) =
            init_oracle(&mut svm, &payer, &eth_mint, &usdc_mint, DEFAULT_CAPACITY);
        let (btc_oracle, init_slot) =
            init_oracle(&mut svm, &payer, &btc_mint, &usdc_mint, DEFAULT_CAPACITY);

        svm.warp_to_slot(init_slot + 10);
        svm.expire_blockhash();

        // Send three independent transactions at the same slot.
        // On mainnet, the scheduler sees:
        //   tx1 write-locks: {sol_oracle, sol_obs}
        //   tx2 write-locks: {eth_oracle, eth_obs}
        //   tx3 write-locks: {btc_oracle, btc_obs}
        // No intersection → all three execute in parallel threads.
        let blockhash = svm.latest_blockhash();

        let tx_sol = Transaction::new_signed_with_payer(
            &[build_update_price_ix(&payer.pubkey(), &sol_oracle, 150)],
            Some(&payer.pubkey()),
            &[&payer],
            blockhash,
        );
        svm.send_transaction(tx_sol).unwrap();

        let tx_eth = Transaction::new_signed_with_payer(
            &[build_update_price_ix(&payer.pubkey(), &eth_oracle, 2500)],
            Some(&payer.pubkey()),
            &[&payer],
            blockhash,
        );
        svm.send_transaction(tx_eth).unwrap();

        let tx_btc = Transaction::new_signed_with_payer(
            &[build_update_price_ix(&payer.pubkey(), &btc_oracle, 60000)],
            Some(&payer.pubkey()),
            &[&payer],
            blockhash,
        );
        svm.send_transaction(tx_btc).unwrap();

        // All three succeeded in the same slot with no contention
        let sol = deserialize_oracle(&svm, &sol_oracle);
        let eth = deserialize_oracle(&svm, &eth_oracle);
        let btc = deserialize_oracle(&svm, &btc_oracle);

        assert_eq!(sol.last_price, 150);
        assert_eq!(eth.last_price, 2500);
        assert_eq!(btc.last_price, 60000);

        // All updated at the same slot
        assert_eq!(sol.last_slot, init_slot + 10);
        assert_eq!(eth.last_slot, init_slot + 10);
        assert_eq!(btc.last_slot, init_slot + 10);
    }

    #[test]
    fn test_parallel_updates_cumulative_price_independent() {
        // Verify that cumulative price accumulation is fully independent
        // across pairs after multiple rounds of parallel updates.

        let mut svm = setup();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();

        let sol_mint = create_mint(&mut svm, &payer);
        let eth_mint = create_mint(&mut svm, &payer);
        let usdc_mint = create_mint(&mut svm, &payer);

        let (sol_oracle, init_slot) =
            init_oracle(&mut svm, &payer, &sol_mint, &usdc_mint, DEFAULT_CAPACITY);
        let (eth_oracle, _) =
            init_oracle(&mut svm, &payer, &eth_mint, &usdc_mint, DEFAULT_CAPACITY);

        // Round 1: set initial prices
        svm.warp_to_slot(init_slot + 10);
        svm.expire_blockhash();
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(
            &[
                build_update_price_ix(&payer.pubkey(), &sol_oracle, 100),
                build_update_price_ix(&payer.pubkey(), &eth_oracle, 2000),
            ],
            Some(&payer.pubkey()),
            &[&payer],
            blockhash,
        );
        svm.send_transaction(tx).unwrap();

        // Round 2: update after 20 more slots
        svm.warp_to_slot(init_slot + 30);
        svm.expire_blockhash();
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(
            &[
                build_update_price_ix(&payer.pubkey(), &sol_oracle, 110),
                build_update_price_ix(&payer.pubkey(), &eth_oracle, 2200),
            ],
            Some(&payer.pubkey()),
            &[&payer],
            blockhash,
        );
        svm.send_transaction(tx).unwrap();

        // Verify cumulative prices are independent
        let sol = deserialize_oracle(&svm, &sol_oracle);
        let eth = deserialize_oracle(&svm, &eth_oracle);

        // SOL: cumul = 0*10 + 100*20 = 2000
        assert_eq!(sol.cumulative_price, 2000);
        assert_eq!(sol.last_price, 110);

        // ETH: cumul = 0*10 + 2000*20 = 40_000
        assert_eq!(eth.cumulative_price, 40_000);
        assert_eq!(eth.last_price, 2200);

        // Get SWAP for each pair over the full 30-slot window
        svm.warp_to_slot(init_slot + 40);
        svm.expire_blockhash();

        // SOL SWAP: cumul_now = 2000 + 110*10 = 3100
        // past obs at slot init_slot+10 (cumul=0)
        // SWAP = 3100 / 30 = 103
        let sol_swap = do_get_swap(&mut svm, &payer, &sol_oracle, 30);
        assert_eq!(sol_swap, 103);

        // ETH SWAP: cumul_now = 40_000 + 2200*10 = 62_000
        // past obs at slot init_slot+10 (cumul=0)
        // SWAP = 62_000 / 30 = 2066
        svm.expire_blockhash();
        let eth_swap = do_get_swap(&mut svm, &payer, &eth_oracle, 30);
        assert_eq!(eth_swap, 2066);
    }

    #[test]
    fn test_50_pairs_concurrent_updates() {
        // Scalability test: 50 trading pairs initialized and updated in the same
        // slot. Demonstrates that the oracle design scales linearly — each pair
        // is an independent account island with zero cross-pair contention.
        //
        // On a real Solana validator, the scheduler would distribute these 50
        // transactions across all available cores because every tx's write-lock
        // set ({oracle_i, obs_buf_i}) is disjoint from all others.

        const NUM_PAIRS: usize = 50;

        let mut svm = setup();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap(); // 100 SOL for rent

        let quote_mint = create_mint(&mut svm, &payer); // shared quote (e.g. USDC)

        // Generate 50 unique base mints and initialize their oracles
        let mut base_mints = Vec::with_capacity(NUM_PAIRS);
        let mut oracle_pdas = Vec::with_capacity(NUM_PAIRS);
        let mut init_slot = 0u64;

        for _ in 0..NUM_PAIRS {
            let base = create_mint(&mut svm, &payer);
            let (oracle, slot) = init_oracle(&mut svm, &payer, &base, &quote_mint, DEFAULT_CAPACITY);
            base_mints.push(base);
            oracle_pdas.push(oracle);
            init_slot = slot;
        }

        // Warp forward so updates are valid
        svm.warp_to_slot(init_slot + 10);
        svm.expire_blockhash();

        // Send all 50 updates as separate transactions in the same slot.
        // Each tx only write-locks its own oracle + observation buffer,
        // so on mainnet all 50 would be parallelized by Sealevel.
        let blockhash = svm.latest_blockhash();
        for (i, oracle) in oracle_pdas.iter().enumerate() {
            let price = ((i + 1) * 100) as u128; // prices: 100, 200, ..., 5000
            let tx = Transaction::new_signed_with_payer(
                &[build_update_price_ix(&payer.pubkey(), oracle, price)],
                Some(&payer.pubkey()),
                &[&payer],
                blockhash,
            );
            svm.send_transaction(tx)
                .unwrap_or_else(|e| panic!("update_price failed for pair {}: {:?}", i, e));
        }

        // Verify all 50 oracles updated correctly and independently
        for (i, oracle_pda) in oracle_pdas.iter().enumerate() {
            let expected_price = ((i + 1) * 100) as u128;
            let oracle = deserialize_oracle(&svm, oracle_pda);

            assert_eq!(
                oracle.last_price, expected_price,
                "Pair {} last_price mismatch", i
            );
            assert_eq!(
                oracle.last_slot,
                init_slot + 10,
                "Pair {} last_slot mismatch", i
            );
            // First update from price=0, so cumulative stays 0
            assert_eq!(
                oracle.cumulative_price, 0,
                "Pair {} cumulative_price should be 0 on first update", i
            );

            // Verify observation buffer got exactly one entry
            let (obs_pda, _) = observation_buffer_pda(oracle_pda);
            let buffer = deserialize_observation_buffer(&svm, &obs_pda);
            assert_eq!(
                buffer.observations.len(), 1,
                "Pair {} should have 1 observation", i
            );
        }

        // Round 2: update all 50 again after 20 more slots
        svm.warp_to_slot(init_slot + 30);
        svm.expire_blockhash();
        let blockhash = svm.latest_blockhash();

        for (i, oracle) in oracle_pdas.iter().enumerate() {
            let price = ((i + 1) * 110) as u128; // new prices: 110, 220, ..., 5500
            let tx = Transaction::new_signed_with_payer(
                &[build_update_price_ix(&payer.pubkey(), oracle, price)],
                Some(&payer.pubkey()),
                &[&payer],
                blockhash,
            );
            svm.send_transaction(tx).unwrap();
        }

        // Verify cumulative math is correct for all 50 pairs
        for (i, oracle_pda) in oracle_pdas.iter().enumerate() {
            let old_price = ((i + 1) * 100) as u128;
            let new_price = ((i + 1) * 110) as u128;
            let oracle = deserialize_oracle(&svm, oracle_pda);

            // cumulative = 0 + old_price * 20
            assert_eq!(
                oracle.cumulative_price,
                old_price * 20,
                "Pair {} cumulative_price mismatch after round 2", i
            );
            assert_eq!(oracle.last_price, new_price);
            assert_eq!(oracle.last_slot, init_slot + 30);

            let (obs_pda, _) = observation_buffer_pda(oracle_pda);
            let buffer = deserialize_observation_buffer(&svm, &obs_pda);
            assert_eq!(buffer.observations.len(), 2, "Pair {} should have 2 observations", i);
        }
    }

    // ── Reward tracking / event tests ──

    /// Helper: warp, send update_price, and return TransactionMetadata for log inspection.
    fn do_update_price_with_meta(
        svm: &mut LiteSVM,
        payer: &Keypair,
        oracle_pda: &Pubkey,
        new_price: u128,
        target_slot: u64,
    ) -> TransactionMetadata {
        svm.warp_to_slot(target_slot);
        svm.expire_blockhash();
        let ix = build_update_price_ix(&payer.pubkey(), oracle_pda, new_price);
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&payer.pubkey()),
            &[payer],
            blockhash,
        );
        svm.send_transaction(tx).expect("update_price failed")
    }

    /// Extract Anchor event data blobs from transaction logs.
    ///
    /// Anchor's `emit!()` writes events via `sol_log_data`, which the runtime
    /// surfaces as `"Program data: <base64>"` log lines. Each blob starts with
    /// the 8-byte event discriminator followed by the borsh payload.
    fn extract_anchor_events(meta: &TransactionMetadata) -> Vec<Vec<u8>> {
        let engine = base64::engine::general_purpose::STANDARD;
        let mut events = Vec::new();
        for log in &meta.logs {
            if let Some(b64) = log.strip_prefix("Program data: ") {
                if let Ok(data) = engine.decode(b64) {
                    if data.len() >= 8 {
                        events.push(data);
                    }
                }
            }
        }
        events
    }

    /// Decode a specific Anchor event from raw log data (discriminator + borsh payload).
    fn decode_event<T: AnchorDeserialize + Discriminator>(data: &[u8]) -> Option<T> {
        let disc = T::DISCRIMINATOR;
        if data.len() < disc.len() || data[..disc.len()] != *disc {
            return None;
        }
        T::deserialize(&mut &data[disc.len()..]).ok()
    }

    #[test]
    fn test_last_updater_set_on_update() {
        let mut svm = setup();
        let payer = Keypair::new();
        let updater = Keypair::new();
        svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();
        svm.airdrop(&updater.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &payer);
        let quote_mint = create_mint(&mut svm, &payer);
        let (oracle_pda, init_slot) =
            init_oracle(&mut svm, &payer, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        // Before any update, last_updater is default (zeroed)
        let oracle = deserialize_oracle(&svm, &oracle_pda);
        assert_eq!(oracle.last_updater, Pubkey::default());

        // Update from updater — last_updater should be set
        do_update_price(&mut svm, &updater, &oracle_pda, 500, init_slot + 10);
        let oracle = deserialize_oracle(&svm, &oracle_pda);
        assert_eq!(oracle.last_updater, updater.pubkey());
    }

    #[test]
    fn test_last_updater_tracks_most_recent_signer() {
        let mut svm = setup();
        let payer = Keypair::new();
        let signer_a = Keypair::new();
        let signer_b = Keypair::new();
        svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();
        svm.airdrop(&signer_a.pubkey(), 10_000_000_000).unwrap();
        svm.airdrop(&signer_b.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &payer);
        let quote_mint = create_mint(&mut svm, &payer);
        let (oracle_pda, init_slot) =
            init_oracle(&mut svm, &payer, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        // A updates
        do_update_price(&mut svm, &signer_a, &oracle_pda, 1000, init_slot + 10);
        let oracle = deserialize_oracle(&svm, &oracle_pda);
        assert_eq!(oracle.last_updater, signer_a.pubkey());

        // B updates — last_updater flips to B
        do_update_price(&mut svm, &signer_b, &oracle_pda, 1100, init_slot + 20);
        let oracle = deserialize_oracle(&svm, &oracle_pda);
        assert_eq!(oracle.last_updater, signer_b.pubkey());
    }

    #[test]
    fn test_oracle_update_event_emitted() {
        let mut svm = setup();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &payer);
        let quote_mint = create_mint(&mut svm, &payer);
        let (oracle_pda, init_slot) =
            init_oracle(&mut svm, &payer, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        let price = 42_000u128;
        let target_slot = init_slot + 10;
        let meta = do_update_price_with_meta(
            &mut svm, &payer, &oracle_pda, price, target_slot,
        );

        let events = extract_anchor_events(&meta);
        let updates: Vec<OracleUpdate> = events
            .iter()
            .filter_map(|e| decode_event::<OracleUpdate>(e))
            .collect();

        assert_eq!(updates.len(), 1, "Expected exactly one OracleUpdate event");
        assert_eq!(updates[0].oracle, oracle_pda);
        assert_eq!(updates[0].price, price);
        assert_eq!(updates[0].cumulative_price, 0); // first update from price=0
        assert_eq!(updates[0].slot, target_slot);
        assert_eq!(updates[0].updater, payer.pubkey());
    }

    #[test]
    fn test_oracle_update_event_fields_consistent_with_state() {
        let mut svm = setup();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &payer);
        let quote_mint = create_mint(&mut svm, &payer);
        let (oracle_pda, init_slot) =
            init_oracle(&mut svm, &payer, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        // First update
        do_update_price(&mut svm, &payer, &oracle_pda, 1000, init_slot + 10);

        // Second update — event should reflect cumulative state
        let target_slot = init_slot + 20;
        let meta = do_update_price_with_meta(
            &mut svm, &payer, &oracle_pda, 1100, target_slot,
        );

        let events = extract_anchor_events(&meta);
        let updates: Vec<OracleUpdate> = events
            .iter()
            .filter_map(|e| decode_event::<OracleUpdate>(e))
            .collect();

        assert_eq!(updates.len(), 1);

        // Verify event matches on-chain state
        let oracle = deserialize_oracle(&svm, &oracle_pda);
        assert_eq!(updates[0].oracle, oracle_pda);
        assert_eq!(updates[0].price, oracle.last_price);
        assert_eq!(updates[0].cumulative_price, oracle.cumulative_price);
        assert_eq!(updates[0].slot, target_slot);
        assert_eq!(updates[0].updater, payer.pubkey());
    }

    // ── Staleness protection tests ──

    #[test]
    fn test_get_swap_fails_when_oracle_is_stale() {
        let mut svm = setup();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &payer);
        let quote_mint = create_mint(&mut svm, &payer);
        let (oracle_pda, init_slot) =
            init_oracle(&mut svm, &payer, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        // Update at init_slot+10
        do_update_price(&mut svm, &payer, &oracle_pda, 500, init_slot + 10);

        // Warp to init_slot+110 — oracle is now 100 slots stale
        svm.warp_to_slot(init_slot + 110);
        svm.expire_blockhash();

        // Request with max_staleness_slots=50 — oracle age (100) exceeds threshold
        let ix = build_get_swap_ix(&oracle_pda, 20, 50);
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&payer.pubkey()),
            &[&payer],
            blockhash,
        );
        let result = svm.send_transaction(tx);
        assert_anchor_error(&result, OracleError::StaleOracle);
    }

    #[test]
    fn test_get_swap_succeeds_within_staleness_threshold() {
        let mut svm = setup();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &payer);
        let quote_mint = create_mint(&mut svm, &payer);
        let (oracle_pda, init_slot) =
            init_oracle(&mut svm, &payer, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        // Update at init_slot+10
        do_update_price(&mut svm, &payer, &oracle_pda, 500, init_slot + 10);

        // Warp to init_slot+30 — oracle is 20 slots stale
        svm.warp_to_slot(init_slot + 30);
        svm.expire_blockhash();

        // max_staleness_slots=20 — exactly at the boundary, should succeed
        let ix = build_get_swap_ix(&oracle_pda, 20, 20);
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&payer.pubkey()),
            &[&payer],
            blockhash,
        );
        let meta = svm.send_transaction(tx).expect("get_swap should succeed at boundary");
        let swap = parse_return_value::<u128>(&meta);
        // cumulative_now = 0 + 500*20 = 10_000, past obs at slot init_slot+10 (cumul=0)
        // SWAP = 10_000 / 20 = 500
        assert_eq!(swap, 500);

        // max_staleness_slots=100 — well within threshold
        svm.expire_blockhash();
        let swap = do_get_swap(&mut svm, &payer, &oracle_pda, 20);
        assert_eq!(swap, 500);
    }

    // ── Price deviation guard tests ──

    #[test]
    fn test_update_price_within_deviation_threshold() {
        let mut svm = setup();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &payer);
        let quote_mint = create_mint(&mut svm, &payer);
        let (oracle_pda, init_slot) =
            init_oracle(&mut svm, &payer, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        // First update (from 0) — always allowed regardless of deviation
        do_update_price(&mut svm, &payer, &oracle_pda, 1000, init_slot + 10);

        // 10% increase (exactly at 1000 bps boundary) — should succeed
        do_update_price(&mut svm, &payer, &oracle_pda, 1100, init_slot + 20);
        let oracle = deserialize_oracle(&svm, &oracle_pda);
        assert_eq!(oracle.last_price, 1100);

        // 10% decrease from 1100 — should succeed
        do_update_price(&mut svm, &payer, &oracle_pda, 990, init_slot + 30);
        let oracle = deserialize_oracle(&svm, &oracle_pda);
        assert_eq!(oracle.last_price, 990);
    }

    #[test]
    fn test_update_price_exceeds_deviation_threshold() {
        let mut svm = setup();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &payer);
        let quote_mint = create_mint(&mut svm, &payer);
        let (oracle_pda, init_slot) =
            init_oracle(&mut svm, &payer, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        do_update_price(&mut svm, &payer, &oracle_pda, 1000, init_slot + 10);

        // 11% increase — exceeds 10% threshold
        svm.warp_to_slot(init_slot + 20);
        svm.expire_blockhash();
        let ix = build_update_price_ix(&payer.pubkey(), &oracle_pda, 1111);
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&payer.pubkey()),
            &[&payer],
            blockhash,
        );
        let result = svm.send_transaction(tx);
        assert_anchor_error(&result, OracleError::PriceDeviationTooLarge);

        // Oracle unchanged
        let oracle = deserialize_oracle(&svm, &oracle_pda);
        assert_eq!(oracle.last_price, 1000);

        // Large decrease — also rejected
        let ix = build_update_price_ix(&payer.pubkey(), &oracle_pda, 800);
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&payer.pubkey()),
            &[&payer],
            blockhash,
        );
        let result = svm.send_transaction(tx);
        assert_anchor_error(&result, OracleError::PriceDeviationTooLarge);
    }

    // ── Ownership transfer tests ──

    fn build_transfer_ownership_ix(
        oracle: &Pubkey,
        owner: &Pubkey,
        new_owner: &Pubkey,
    ) -> Instruction {
        let data = slot_twap_oracle::instruction::TransferOwnership.data();

        Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(*oracle, false),
                AccountMeta::new_readonly(*owner, true),
                AccountMeta::new_readonly(*new_owner, false),
            ],
            data,
        }
    }

    #[test]
    fn test_transfer_ownership_success() {
        let mut svm = setup();
        let owner = Keypair::new();
        let new_owner = Keypair::new();
        svm.airdrop(&owner.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &owner);
        let quote_mint = create_mint(&mut svm, &owner);
        let (oracle_pda, _) =
            init_oracle(&mut svm, &owner, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        // Owner is the initializer
        let oracle = deserialize_oracle(&svm, &oracle_pda);
        assert_eq!(oracle.owner, owner.pubkey());

        // Transfer ownership
        let ix = build_transfer_ownership_ix(&oracle_pda, &owner.pubkey(), &new_owner.pubkey());
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&owner.pubkey()),
            &[&owner],
            blockhash,
        );
        svm.send_transaction(tx).unwrap();

        // Verify new owner
        let oracle = deserialize_oracle(&svm, &oracle_pda);
        assert_eq!(oracle.owner, new_owner.pubkey());
    }

    #[test]
    fn test_transfer_ownership_non_owner_fails() {
        let mut svm = setup();
        let owner = Keypair::new();
        let attacker = Keypair::new();
        let new_owner = Keypair::new();
        svm.airdrop(&owner.pubkey(), 10_000_000_000).unwrap();
        svm.airdrop(&attacker.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &owner);
        let quote_mint = create_mint(&mut svm, &owner);
        let (oracle_pda, _) =
            init_oracle(&mut svm, &owner, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        // Non-owner tries to transfer — should fail
        let ix = build_transfer_ownership_ix(&oracle_pda, &attacker.pubkey(), &new_owner.pubkey());
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&attacker.pubkey()),
            &[&attacker],
            blockhash,
        );
        let result = svm.send_transaction(tx);
        assert!(result.is_err(), "Non-owner should not be able to transfer ownership");

        // Owner unchanged
        let oracle = deserialize_oracle(&svm, &oracle_pda);
        assert_eq!(oracle.owner, owner.pubkey());
    }

    #[test]
    fn test_transfer_ownership_to_same_owner_fails() {
        let mut svm = setup();
        let owner = Keypair::new();
        svm.airdrop(&owner.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &owner);
        let quote_mint = create_mint(&mut svm, &owner);
        let (oracle_pda, _) =
            init_oracle(&mut svm, &owner, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        // Transfer to self — should fail
        let ix = build_transfer_ownership_ix(&oracle_pda, &owner.pubkey(), &owner.pubkey());
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&owner.pubkey()),
            &[&owner],
            blockhash,
        );
        let result = svm.send_transaction(tx);
        assert_anchor_error(&result, OracleError::Unauthorized);
    }

    // ── Pause/unpause tests ──

    fn build_set_paused_ix(
        oracle: &Pubkey,
        owner: &Pubkey,
        paused: bool,
    ) -> Instruction {
        let data = slot_twap_oracle::instruction::SetPaused { paused }.data();

        Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(*oracle, false),
                AccountMeta::new_readonly(*owner, true),
            ],
            data,
        }
    }

    #[test]
    fn test_pause_and_unpause_oracle() {
        let mut svm = setup();
        let owner = Keypair::new();
        svm.airdrop(&owner.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &owner);
        let quote_mint = create_mint(&mut svm, &owner);
        let (oracle_pda, _) =
            init_oracle(&mut svm, &owner, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        // Initially not paused
        let oracle = deserialize_oracle(&svm, &oracle_pda);
        assert!(!oracle.paused);

        // Pause
        let ix = build_set_paused_ix(&oracle_pda, &owner.pubkey(), true);
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&owner.pubkey()),
            &[&owner],
            blockhash,
        );
        svm.send_transaction(tx).unwrap();
        let oracle = deserialize_oracle(&svm, &oracle_pda);
        assert!(oracle.paused);

        // Unpause
        let ix = build_set_paused_ix(&oracle_pda, &owner.pubkey(), false);
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&owner.pubkey()),
            &[&owner],
            blockhash,
        );
        svm.send_transaction(tx).unwrap();
        let oracle = deserialize_oracle(&svm, &oracle_pda);
        assert!(!oracle.paused);
    }

    #[test]
    fn test_pause_blocks_update_price() {
        let mut svm = setup();
        let owner = Keypair::new();
        svm.airdrop(&owner.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &owner);
        let quote_mint = create_mint(&mut svm, &owner);
        let (oracle_pda, init_slot) =
            init_oracle(&mut svm, &owner, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        // Pause the oracle
        let ix = build_set_paused_ix(&oracle_pda, &owner.pubkey(), true);
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&owner.pubkey()),
            &[&owner],
            blockhash,
        );
        svm.send_transaction(tx).unwrap();

        // update_price should fail
        svm.warp_to_slot(init_slot + 10);
        svm.expire_blockhash();
        let ix = build_update_price_ix(&owner.pubkey(), &oracle_pda, 1000);
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&owner.pubkey()),
            &[&owner],
            blockhash,
        );
        let result = svm.send_transaction(tx);
        assert_anchor_error(&result, OracleError::OraclePaused);
    }

    #[test]
    fn test_pause_blocks_get_swap() {
        let mut svm = setup();
        let owner = Keypair::new();
        svm.airdrop(&owner.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &owner);
        let quote_mint = create_mint(&mut svm, &owner);
        let (oracle_pda, init_slot) =
            init_oracle(&mut svm, &owner, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        // Add an observation so get_swap would otherwise succeed
        do_update_price(&mut svm, &owner, &oracle_pda, 1000, init_slot + 10);

        // Pause
        let ix = build_set_paused_ix(&oracle_pda, &owner.pubkey(), true);
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&owner.pubkey()),
            &[&owner],
            blockhash,
        );
        svm.send_transaction(tx).unwrap();

        // get_swap should fail
        let ix = build_get_swap_ix(&oracle_pda, 5, DEFAULT_MAX_STALENESS);
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&owner.pubkey()),
            &[&owner],
            blockhash,
        );
        let result = svm.send_transaction(tx);
        assert_anchor_error(&result, OracleError::OraclePaused);
    }

    #[test]
    fn test_non_owner_cannot_pause() {
        let mut svm = setup();
        let owner = Keypair::new();
        let attacker = Keypair::new();
        svm.airdrop(&owner.pubkey(), 10_000_000_000).unwrap();
        svm.airdrop(&attacker.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &owner);
        let quote_mint = create_mint(&mut svm, &owner);
        let (oracle_pda, _) =
            init_oracle(&mut svm, &owner, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        // Non-owner tries to pause
        let ix = build_set_paused_ix(&oracle_pda, &attacker.pubkey(), true);
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&attacker.pubkey()),
            &[&attacker],
            blockhash,
        );
        let result = svm.send_transaction(tx);
        assert!(result.is_err(), "Non-owner should not be able to pause");

        let oracle = deserialize_oracle(&svm, &oracle_pda);
        assert!(!oracle.paused);
    }

    #[test]
    fn test_unpause_allows_updates_again() {
        let mut svm = setup();
        let owner = Keypair::new();
        svm.airdrop(&owner.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &owner);
        let quote_mint = create_mint(&mut svm, &owner);
        let (oracle_pda, init_slot) =
            init_oracle(&mut svm, &owner, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        // Pause
        let ix = build_set_paused_ix(&oracle_pda, &owner.pubkey(), true);
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&owner.pubkey()),
            &[&owner],
            blockhash,
        );
        svm.send_transaction(tx).unwrap();

        // Unpause
        let ix = build_set_paused_ix(&oracle_pda, &owner.pubkey(), false);
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&owner.pubkey()),
            &[&owner],
            blockhash,
        );
        svm.send_transaction(tx).unwrap();

        // update_price should work again
        do_update_price(&mut svm, &owner, &oracle_pda, 1000, init_slot + 10);
        let oracle = deserialize_oracle(&svm, &oracle_pda);
        assert_eq!(oracle.last_price, 1000);
    }
}
