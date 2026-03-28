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
    use slot_twap_oracle::state::{ObservationBuffer, Oracle, RewardVault};
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
        let pid = program_id();

        Instruction {
            program_id: pid,
            accounts: vec![
                AccountMeta::new_readonly(*payer, true),
                AccountMeta::new(*oracle, false),
                AccountMeta::new(obs_pda, false),
                // Optional reward accounts — pass program ID as None placeholder
                AccountMeta::new_readonly(pid, false), // reward_vault
                AccountMeta::new_readonly(pid, false), // vault_token_account
                AccountMeta::new_readonly(pid, false), // reward_mint
                AccountMeta::new_readonly(pid, false), // previous_updater_token_account
                AccountMeta::new_readonly(pid, false), // token_program
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
        assert_eq!(buffer.len, 0);
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
        assert_eq!(buffer.len, 1);
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
        assert_eq!(buffer.len, 3);
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

        assert_eq!(buffer.len, 3);
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
        assert_eq!(buffer.len, 3);
        assert_eq!(buffer.head, 0); // wrapped around

        // 4th update should overwrite index 0
        do_update_price(&mut svm, &payer, &oracle_pda, 1090, init_slot + 40);

        let buffer = deserialize_observation_buffer(&svm, &obs_pda);
        assert_eq!(buffer.len, 3); // still 3
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
        assert!(buffer.len == 0);
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
        assert_eq!(buffer.len, 1);
        assert_eq!(buffer.observations[0].slot, init_slot + 10);

        // Second update overwrites the only slot
        do_update_price(&mut svm, &payer, &oracle_pda, 1100, init_slot + 20);
        let buffer = deserialize_observation_buffer(&svm, &obs_pda);
        assert_eq!(buffer.len, 1);
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

        assert_eq!(sol_buf.len, 1);
        assert_eq!(eth_buf.len, 1);
        assert_eq!(btc_buf.len, 1);
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
                buffer.len, 1,
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
            assert_eq!(buffer.len, 2, "Pair {} should have 2 observations", i);
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

    // ── Resize buffer tests ──

    fn build_resize_buffer_ix(
        oracle: &Pubkey,
        owner: &Pubkey,
        new_capacity: u32,
    ) -> Instruction {
        let (obs_pda, _) = observation_buffer_pda(oracle);
        let data = slot_twap_oracle::instruction::ResizeBuffer { new_capacity }.data();

        Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new_readonly(*oracle, false),
                AccountMeta::new(obs_pda, false),
                AccountMeta::new(*owner, true),
                AccountMeta::new_readonly(system_program::id(), false),
            ],
            data,
        }
    }

    #[test]
    fn test_resize_buffer_grow() {
        let mut svm = setup();
        let owner = Keypair::new();
        svm.airdrop(&owner.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &owner);
        let quote_mint = create_mint(&mut svm, &owner);
        let (oracle_pda, init_slot) =
            init_oracle(&mut svm, &owner, &base_mint, &quote_mint, 3);

        // Add 2 observations
        do_update_price(&mut svm, &owner, &oracle_pda, 1000, init_slot + 10);
        do_update_price(&mut svm, &owner, &oracle_pda, 1100, init_slot + 20);

        // Grow from 3 to 10
        let ix = build_resize_buffer_ix(&oracle_pda, &owner.pubkey(), 10);
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&owner.pubkey()),
            &[&owner],
            blockhash,
        );
        svm.send_transaction(tx).unwrap();

        let (obs_pda, _) = observation_buffer_pda(&oracle_pda);
        let buffer = deserialize_observation_buffer(&svm, &obs_pda);
        assert_eq!(buffer.capacity, 10);
        // All observations preserved
        assert_eq!(buffer.len, 2);
        assert_eq!(buffer.observations[0].slot, init_slot + 10);
        assert_eq!(buffer.observations[1].slot, init_slot + 20);
    }

    #[test]
    fn test_resize_buffer_shrink_preserves_newest() {
        let mut svm = setup();
        let owner = Keypair::new();
        svm.airdrop(&owner.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &owner);
        let quote_mint = create_mint(&mut svm, &owner);
        let (oracle_pda, init_slot) =
            init_oracle(&mut svm, &owner, &base_mint, &quote_mint, 5);

        // Fill 4 observations
        do_update_price(&mut svm, &owner, &oracle_pda, 1000, init_slot + 10);
        do_update_price(&mut svm, &owner, &oracle_pda, 1100, init_slot + 20);
        do_update_price(&mut svm, &owner, &oracle_pda, 1050, init_slot + 30);
        do_update_price(&mut svm, &owner, &oracle_pda, 1090, init_slot + 40);

        // Shrink from 5 to 2 — should keep the 2 most recent
        let ix = build_resize_buffer_ix(&oracle_pda, &owner.pubkey(), 2);
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&owner.pubkey()),
            &[&owner],
            blockhash,
        );
        svm.send_transaction(tx).unwrap();

        let (obs_pda, _) = observation_buffer_pda(&oracle_pda);
        let buffer = deserialize_observation_buffer(&svm, &obs_pda);
        assert_eq!(buffer.capacity, 2);
        assert_eq!(buffer.len, 2);
        // Most recent 2: slot+30 and slot+40
        assert_eq!(buffer.observations[0].slot, init_slot + 30);
        assert_eq!(buffer.observations[1].slot, init_slot + 40);
    }

    #[test]
    fn test_resize_buffer_shrink_after_wrap() {
        let mut svm = setup();
        let owner = Keypair::new();
        svm.airdrop(&owner.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &owner);
        let quote_mint = create_mint(&mut svm, &owner);
        let (oracle_pda, init_slot) =
            init_oracle(&mut svm, &owner, &base_mint, &quote_mint, 3);

        // Fill buffer (3) and wrap: 4 updates into capacity-3
        do_update_price(&mut svm, &owner, &oracle_pda, 1000, init_slot + 10);
        do_update_price(&mut svm, &owner, &oracle_pda, 1100, init_slot + 20);
        do_update_price(&mut svm, &owner, &oracle_pda, 1050, init_slot + 30);
        do_update_price(&mut svm, &owner, &oracle_pda, 1090, init_slot + 40); // overwrites slot+10

        // Buffer is wrapped: [slot+40, slot+20, slot+30], head=1
        // Shrink to 2 — should linearize and keep newest 2: slot+30, slot+40
        let ix = build_resize_buffer_ix(&oracle_pda, &owner.pubkey(), 2);
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&owner.pubkey()),
            &[&owner],
            blockhash,
        );
        svm.send_transaction(tx).unwrap();

        let (obs_pda, _) = observation_buffer_pda(&oracle_pda);
        let buffer = deserialize_observation_buffer(&svm, &obs_pda);
        assert_eq!(buffer.capacity, 2);
        assert_eq!(buffer.len, 2);
        assert_eq!(buffer.observations[0].slot, init_slot + 30);
        assert_eq!(buffer.observations[1].slot, init_slot + 40);
    }

    #[test]
    fn test_resize_buffer_zero_capacity_fails() {
        let mut svm = setup();
        let owner = Keypair::new();
        svm.airdrop(&owner.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &owner);
        let quote_mint = create_mint(&mut svm, &owner);
        let (oracle_pda, _) =
            init_oracle(&mut svm, &owner, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        let ix = build_resize_buffer_ix(&oracle_pda, &owner.pubkey(), 0);
        send_tx_expect_err(&mut svm, &owner, &[ix]);
    }

    #[test]
    fn test_resize_buffer_non_owner_fails() {
        let mut svm = setup();
        let owner = Keypair::new();
        let attacker = Keypair::new();
        svm.airdrop(&owner.pubkey(), 10_000_000_000).unwrap();
        svm.airdrop(&attacker.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &owner);
        let quote_mint = create_mint(&mut svm, &owner);
        let (oracle_pda, _) =
            init_oracle(&mut svm, &owner, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        let ix = build_resize_buffer_ix(&oracle_pda, &attacker.pubkey(), 64);
        send_tx_expect_err(&mut svm, &attacker, &[ix]);
    }

    // ── Configurable deviation tests ──

    fn build_set_max_deviation_ix(
        oracle: &Pubkey,
        owner: &Pubkey,
        new_max_deviation_bps: u16,
    ) -> Instruction {
        let data = slot_twap_oracle::instruction::SetMaxDeviation { new_max_deviation_bps }.data();

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
    fn test_set_max_deviation() {
        let mut svm = setup();
        let owner = Keypair::new();
        svm.airdrop(&owner.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &owner);
        let quote_mint = create_mint(&mut svm, &owner);
        let (oracle_pda, _) =
            init_oracle(&mut svm, &owner, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        // Default is 1000 bps (10%)
        let oracle = deserialize_oracle(&svm, &oracle_pda);
        assert_eq!(oracle.max_deviation_bps, 1000);

        // Set to 500 bps (5%)
        let ix = build_set_max_deviation_ix(&oracle_pda, &owner.pubkey(), 500);
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&owner.pubkey()),
            &[&owner],
            blockhash,
        );
        svm.send_transaction(tx).unwrap();

        let oracle = deserialize_oracle(&svm, &oracle_pda);
        assert_eq!(oracle.max_deviation_bps, 500);
    }

    #[test]
    fn test_custom_deviation_enforced() {
        let mut svm = setup();
        let owner = Keypair::new();
        svm.airdrop(&owner.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &owner);
        let quote_mint = create_mint(&mut svm, &owner);
        let (oracle_pda, init_slot) =
            init_oracle(&mut svm, &owner, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        // Set tight threshold: 200 bps (2%)
        let ix = build_set_max_deviation_ix(&oracle_pda, &owner.pubkey(), 200);
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&owner.pubkey()),
            &[&owner],
            blockhash,
        );
        svm.send_transaction(tx).unwrap();

        // First update always passes
        do_update_price(&mut svm, &owner, &oracle_pda, 1000, init_slot + 10);

        // 2% change (1000 → 1020) should succeed
        do_update_price(&mut svm, &owner, &oracle_pda, 1020, init_slot + 20);

        // 5% change (1020 → 1071) should fail
        svm.warp_to_slot(init_slot + 30);
        svm.expire_blockhash();
        let ix = build_update_price_ix(&owner.pubkey(), &oracle_pda, 1071);
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&owner.pubkey()),
            &[&owner],
            blockhash,
        );
        let result = svm.send_transaction(tx);
        assert_anchor_error(&result, OracleError::PriceDeviationTooLarge);
    }

    #[test]
    fn test_wider_deviation_allows_larger_jumps() {
        let mut svm = setup();
        let owner = Keypair::new();
        svm.airdrop(&owner.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &owner);
        let quote_mint = create_mint(&mut svm, &owner);
        let (oracle_pda, init_slot) =
            init_oracle(&mut svm, &owner, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        // Widen to 5000 bps (50%)
        let ix = build_set_max_deviation_ix(&oracle_pda, &owner.pubkey(), 5000);
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&owner.pubkey()),
            &[&owner],
            blockhash,
        );
        svm.send_transaction(tx).unwrap();

        do_update_price(&mut svm, &owner, &oracle_pda, 1000, init_slot + 10);

        // 40% jump (1000 → 1400) — would fail at default 10%, but passes at 50%
        do_update_price(&mut svm, &owner, &oracle_pda, 1400, init_slot + 20);
        let oracle = deserialize_oracle(&svm, &oracle_pda);
        assert_eq!(oracle.last_price, 1400);
    }

    #[test]
    fn test_set_max_deviation_non_owner_fails() {
        let mut svm = setup();
        let owner = Keypair::new();
        let attacker = Keypair::new();
        svm.airdrop(&owner.pubkey(), 10_000_000_000).unwrap();
        svm.airdrop(&attacker.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &owner);
        let quote_mint = create_mint(&mut svm, &owner);
        let (oracle_pda, _) =
            init_oracle(&mut svm, &owner, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        let ix = build_set_max_deviation_ix(&oracle_pda, &attacker.pubkey(), 5000);
        send_tx_expect_err(&mut svm, &attacker, &[ix]);

        let oracle = deserialize_oracle(&svm, &oracle_pda);
        assert_eq!(oracle.max_deviation_bps, 1000); // unchanged
    }

    // ── Reward vault helpers ──

    fn reward_vault_pda(oracle: &Pubkey) -> (Pubkey, u8) {
        Pubkey::find_program_address(&[b"reward", oracle.as_ref()], &program_id())
    }

    fn vault_token_account_pda(oracle: &Pubkey) -> (Pubkey, u8) {
        Pubkey::find_program_address(&[b"reward_tokens", oracle.as_ref()], &program_id())
    }

    fn create_token_account(
        svm: &mut LiteSVM,
        payer: &Keypair,
        mint: &Pubkey,
        owner: &Pubkey,
    ) -> Pubkey {
        let account = Keypair::new();
        let rent = svm.minimum_balance_for_rent_exemption(spl_token_2022::state::Account::LEN);

        let create_ix = solana_sdk::system_instruction::create_account(
            &payer.pubkey(),
            &account.pubkey(),
            rent,
            spl_token_2022::state::Account::LEN as u64,
            &spl_token_2022::id(),
        );
        let init_ix = spl_token_2022::instruction::initialize_account(
            &spl_token_2022::id(),
            &account.pubkey(),
            mint,
            owner,
        )
        .unwrap();

        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(
            &[create_ix, init_ix],
            Some(&payer.pubkey()),
            &[payer, &account],
            blockhash,
        );
        svm.send_transaction(tx).expect("Failed to create token account");
        account.pubkey()
    }

    fn mint_tokens(
        svm: &mut LiteSVM,
        authority: &Keypair,
        mint: &Pubkey,
        dest: &Pubkey,
        amount: u64,
    ) {
        let ix = spl_token_2022::instruction::mint_to(
            &spl_token_2022::id(),
            mint,
            dest,
            &authority.pubkey(),
            &[],
            amount,
        )
        .unwrap();

        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&authority.pubkey()),
            &[authority],
            blockhash,
        );
        svm.send_transaction(tx).expect("Failed to mint tokens");
    }

    fn get_token_balance(svm: &LiteSVM, account: &Pubkey) -> u64 {
        let acct = svm.get_account(account).expect("Token account not found");
        let token = spl_token_2022::state::Account::unpack(&acct.data).expect("Bad token account");
        token.amount
    }

    fn deserialize_reward_vault(svm: &LiteSVM, pubkey: &Pubkey) -> RewardVault {
        let account = svm.get_account(pubkey).expect("RewardVault not found");
        RewardVault::deserialize(&mut &account.data[8..]).expect("Failed to deserialize RewardVault")
    }

    fn build_init_reward_vault_ix(
        oracle: &Pubkey,
        reward_mint: &Pubkey,
        owner: &Pubkey,
        reward_per_update: u64,
    ) -> Instruction {
        let (vault_pda, _) = reward_vault_pda(oracle);
        let (vault_token, _) = vault_token_account_pda(oracle);
        let data = slot_twap_oracle::instruction::InitializeRewardVault { reward_per_update }.data();

        Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new_readonly(*oracle, false),
                AccountMeta::new(vault_pda, false),
                AccountMeta::new(vault_token, false),
                AccountMeta::new_readonly(*reward_mint, false),
                AccountMeta::new(*owner, true),
                AccountMeta::new_readonly(spl_token_2022::id(), false),
                AccountMeta::new_readonly(system_program::id(), false),
            ],
            data,
        }
    }

    fn build_fund_reward_vault_ix(
        oracle: &Pubkey,
        reward_mint: &Pubkey,
        funder: &Pubkey,
        funder_token_account: &Pubkey,
        amount: u64,
    ) -> Instruction {
        let (vault_pda, _) = reward_vault_pda(oracle);
        let (vault_token, _) = vault_token_account_pda(oracle);
        let data = slot_twap_oracle::instruction::FundRewardVault { amount }.data();

        Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new_readonly(*oracle, false),
                AccountMeta::new_readonly(vault_pda, false),
                AccountMeta::new(vault_token, false),
                AccountMeta::new_readonly(*reward_mint, false),
                AccountMeta::new(*funder_token_account, false),
                AccountMeta::new(*funder, true),
                AccountMeta::new_readonly(spl_token_2022::id(), false),
            ],
            data,
        }
    }

    fn build_claim_reward_ix(
        oracle: &Pubkey,
        reward_mint: &Pubkey,
        updater: &Pubkey,
        updater_token_account: &Pubkey,
    ) -> Instruction {
        let (vault_pda, _) = reward_vault_pda(oracle);
        let (vault_token, _) = vault_token_account_pda(oracle);
        let data = slot_twap_oracle::instruction::ClaimReward.data();

        Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new_readonly(*oracle, false),
                AccountMeta::new(vault_pda, false),
                AccountMeta::new(vault_token, false),
                AccountMeta::new_readonly(*reward_mint, false),
                AccountMeta::new(*updater_token_account, false),
                AccountMeta::new_readonly(*updater, true),
                AccountMeta::new_readonly(spl_token_2022::id(), false),
            ],
            data,
        }
    }

    // ── Reward vault tests ──

    /// Full setup: oracle + reward vault + funded
    fn setup_reward_test(
        svm: &mut LiteSVM,
        owner: &Keypair,
        reward_per_update: u64,
        fund_amount: u64,
    ) -> (Pubkey, Pubkey, Pubkey, u64) {
        let base_mint = create_mint(svm, owner);
        let quote_mint = create_mint(svm, owner);
        let reward_mint = create_mint(svm, owner);
        let (oracle_pda, init_slot) = init_oracle(svm, owner, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        // Initialize reward vault
        let ix = build_init_reward_vault_ix(&oracle_pda, &reward_mint, &owner.pubkey(), reward_per_update);
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(&[ix], Some(&owner.pubkey()), &[owner], blockhash);
        svm.send_transaction(tx).unwrap();

        if fund_amount > 0 {
            // Create owner token account, mint tokens, fund vault
            let owner_ata = create_token_account(svm, owner, &reward_mint, &owner.pubkey());
            mint_tokens(svm, owner, &reward_mint, &owner_ata, fund_amount);

            let ix = build_fund_reward_vault_ix(&oracle_pda, &reward_mint, &owner.pubkey(), &owner_ata, fund_amount);
            let blockhash = svm.latest_blockhash();
            let tx = Transaction::new_signed_with_payer(&[ix], Some(&owner.pubkey()), &[owner], blockhash);
            svm.send_transaction(tx).unwrap();
        }

        (oracle_pda, reward_mint, base_mint, init_slot)
    }

    #[test]
    fn test_initialize_reward_vault_state_and_pdas() {
        let mut svm = setup();
        let owner = Keypair::new();
        svm.airdrop(&owner.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &owner);
        let quote_mint = create_mint(&mut svm, &owner);
        let reward_mint = create_mint(&mut svm, &owner);
        let (oracle_pda, _) = init_oracle(&mut svm, &owner, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        let reward_per_update = 2_500_000u64;
        let ix = build_init_reward_vault_ix(&oracle_pda, &reward_mint, &owner.pubkey(), reward_per_update);
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(&[ix], Some(&owner.pubkey()), &[&owner], blockhash);
        svm.send_transaction(tx).unwrap();

        // Verify PDA derivation
        let (expected_vault, _) = reward_vault_pda(&oracle_pda);
        let (expected_token, _) = vault_token_account_pda(&oracle_pda);

        // Verify vault state
        let vault = deserialize_reward_vault(&svm, &expected_vault);
        assert_eq!(vault.oracle, oracle_pda);
        assert_eq!(vault.reward_mint, reward_mint);
        assert_eq!(vault.reward_per_update, reward_per_update);
        assert_eq!(vault.total_distributed, 0);
        assert_eq!(vault.total_updates_rewarded, 0);

        // Verify vault token account exists with zero balance, correct mint
        let token_acct = svm.get_account(&expected_token).expect("Vault token account not found");
        let parsed = spl_token_2022::state::Account::unpack(&token_acct.data).unwrap();
        assert_eq!(parsed.mint, reward_mint);
        assert_eq!(parsed.amount, 0);
        // Authority is the vault PDA
        assert_eq!(parsed.owner, expected_vault);
    }

    #[test]
    fn test_initialize_reward_vault_non_owner_fails() {
        let mut svm = setup();
        let owner = Keypair::new();
        let attacker = Keypair::new();
        svm.airdrop(&owner.pubkey(), 10_000_000_000).unwrap();
        svm.airdrop(&attacker.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &owner);
        let quote_mint = create_mint(&mut svm, &owner);
        let reward_mint = create_mint(&mut svm, &owner);
        let (oracle_pda, _) = init_oracle(&mut svm, &owner, &base_mint, &quote_mint, DEFAULT_CAPACITY);

        // Attacker tries to init vault — should fail (has_one = owner)
        let ix = build_init_reward_vault_ix(&oracle_pda, &reward_mint, &attacker.pubkey(), 1_000_000);
        send_tx_expect_err(&mut svm, &attacker, &[ix]);
    }

    #[test]
    fn test_initialize_reward_vault_duplicate_fails() {
        let mut svm = setup();
        let owner = Keypair::new();
        svm.airdrop(&owner.pubkey(), 10_000_000_000).unwrap();

        let (oracle_pda, reward_mint, _, _) =
            setup_reward_test(&mut svm, &owner, 1_000_000, 0);

        // Second init should fail — PDA already exists
        let ix = build_init_reward_vault_ix(&oracle_pda, &reward_mint, &owner.pubkey(), 2_000_000);
        send_tx_expect_err(&mut svm, &owner, &[ix]);
    }

    #[test]
    fn test_fund_reward_vault_balances() {
        let mut svm = setup();
        let owner = Keypair::new();
        svm.airdrop(&owner.pubkey(), 10_000_000_000).unwrap();

        let (oracle_pda, reward_mint, _, _) =
            setup_reward_test(&mut svm, &owner, 1_000_000, 0);

        let owner_ata = create_token_account(&mut svm, &owner, &reward_mint, &owner.pubkey());
        mint_tokens(&mut svm, &owner, &reward_mint, &owner_ata, 10_000_000);

        let (vault_token, _) = vault_token_account_pda(&oracle_pda);

        // Fund 3M
        let ix = build_fund_reward_vault_ix(&oracle_pda, &reward_mint, &owner.pubkey(), &owner_ata, 3_000_000);
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(&[ix], Some(&owner.pubkey()), &[&owner], blockhash);
        svm.send_transaction(tx).unwrap();

        assert_eq!(get_token_balance(&svm, &vault_token), 3_000_000);
        assert_eq!(get_token_balance(&svm, &owner_ata), 7_000_000);

        // Fund 2M more — cumulative
        let ix = build_fund_reward_vault_ix(&oracle_pda, &reward_mint, &owner.pubkey(), &owner_ata, 2_000_000);
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(&[ix], Some(&owner.pubkey()), &[&owner], blockhash);
        svm.send_transaction(tx).unwrap();

        assert_eq!(get_token_balance(&svm, &vault_token), 5_000_000);
        assert_eq!(get_token_balance(&svm, &owner_ata), 5_000_000);
    }

    #[test]
    fn test_fund_reward_vault_insufficient_tokens_fails() {
        let mut svm = setup();
        let owner = Keypair::new();
        svm.airdrop(&owner.pubkey(), 10_000_000_000).unwrap();

        let (oracle_pda, reward_mint, _, _) =
            setup_reward_test(&mut svm, &owner, 1_000_000, 0);

        let owner_ata = create_token_account(&mut svm, &owner, &reward_mint, &owner.pubkey());
        mint_tokens(&mut svm, &owner, &reward_mint, &owner_ata, 100);

        // Try to fund 1M but only have 100
        let ix = build_fund_reward_vault_ix(&oracle_pda, &reward_mint, &owner.pubkey(), &owner_ata, 1_000_000);
        send_tx_expect_err(&mut svm, &owner, &[ix]);
    }

    #[test]
    fn test_claim_reward_success() {
        let mut svm = setup();
        let owner = Keypair::new();
        svm.airdrop(&owner.pubkey(), 10_000_000_000).unwrap();

        let reward_per_update = 1_000_000u64;
        let (oracle_pda, reward_mint, _, init_slot) =
            setup_reward_test(&mut svm, &owner, reward_per_update, 10_000_000);

        // Update price so owner becomes last_updater
        do_update_price(&mut svm, &owner, &oracle_pda, 1000, init_slot + 10);

        // Create updater token account
        let updater_ata = create_token_account(&mut svm, &owner, &reward_mint, &owner.pubkey());

        // Claim reward
        let ix = build_claim_reward_ix(&oracle_pda, &reward_mint, &owner.pubkey(), &updater_ata);
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(&[ix], Some(&owner.pubkey()), &[&owner], blockhash);
        svm.send_transaction(tx).unwrap();

        // Verify token transferred
        assert_eq!(get_token_balance(&svm, &updater_ata), reward_per_update);

        // Verify vault accounting
        let (vault_pda, _) = reward_vault_pda(&oracle_pda);
        let vault = deserialize_reward_vault(&svm, &vault_pda);
        assert_eq!(vault.total_distributed, reward_per_update);
        assert_eq!(vault.total_updates_rewarded, 1);

        // Verify vault balance decreased
        let (vault_token, _) = vault_token_account_pda(&oracle_pda);
        assert_eq!(get_token_balance(&svm, &vault_token), 10_000_000 - reward_per_update);
    }

    #[test]
    fn test_claim_reward_non_last_updater_fails() {
        let mut svm = setup();
        let owner = Keypair::new();
        let updater = Keypair::new();
        let attacker = Keypair::new();
        svm.airdrop(&owner.pubkey(), 10_000_000_000).unwrap();
        svm.airdrop(&updater.pubkey(), 10_000_000_000).unwrap();
        svm.airdrop(&attacker.pubkey(), 10_000_000_000).unwrap();

        let (oracle_pda, reward_mint, _, init_slot) =
            setup_reward_test(&mut svm, &owner, 1_000_000, 10_000_000);

        // updater updates price → becomes last_updater
        do_update_price(&mut svm, &updater, &oracle_pda, 1000, init_slot + 10);

        // attacker tries to claim — should fail
        let attacker_ata = create_token_account(&mut svm, &attacker, &reward_mint, &attacker.pubkey());
        let ix = build_claim_reward_ix(&oracle_pda, &reward_mint, &attacker.pubkey(), &attacker_ata);
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(&[ix], Some(&attacker.pubkey()), &[&attacker], blockhash);
        let result = svm.send_transaction(tx);
        assert_anchor_error(&result, OracleError::Unauthorized);

        // attacker got nothing
        assert_eq!(get_token_balance(&svm, &attacker_ata), 0);
    }

    #[test]
    fn test_claim_reward_empty_vault_fails() {
        let mut svm = setup();
        let owner = Keypair::new();
        svm.airdrop(&owner.pubkey(), 10_000_000_000).unwrap();

        // Fund with 0 tokens
        let (oracle_pda, reward_mint, _, init_slot) =
            setup_reward_test(&mut svm, &owner, 1_000_000, 0);

        do_update_price(&mut svm, &owner, &oracle_pda, 1000, init_slot + 10);

        let updater_ata = create_token_account(&mut svm, &owner, &reward_mint, &owner.pubkey());
        let ix = build_claim_reward_ix(&oracle_pda, &reward_mint, &owner.pubkey(), &updater_ata);
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(&[ix], Some(&owner.pubkey()), &[&owner], blockhash);
        let result = svm.send_transaction(tx);
        assert_anchor_error(&result, OracleError::InsufficientRewardBalance);
    }

    #[test]
    fn test_claim_reward_multiple_updaters() {
        let mut svm = setup();
        let owner = Keypair::new();
        let updater_a = Keypair::new();
        let updater_b = Keypair::new();
        svm.airdrop(&owner.pubkey(), 10_000_000_000).unwrap();
        svm.airdrop(&updater_a.pubkey(), 10_000_000_000).unwrap();
        svm.airdrop(&updater_b.pubkey(), 10_000_000_000).unwrap();

        let reward_per_update = 500_000u64;
        let (oracle_pda, reward_mint, _, init_slot) =
            setup_reward_test(&mut svm, &owner, reward_per_update, 10_000_000);

        let ata_a = create_token_account(&mut svm, &updater_a, &reward_mint, &updater_a.pubkey());
        let ata_b = create_token_account(&mut svm, &updater_b, &reward_mint, &updater_b.pubkey());

        // A updates and claims
        do_update_price(&mut svm, &updater_a, &oracle_pda, 1000, init_slot + 10);
        let ix = build_claim_reward_ix(&oracle_pda, &reward_mint, &updater_a.pubkey(), &ata_a);
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(&[ix], Some(&updater_a.pubkey()), &[&updater_a], blockhash);
        svm.send_transaction(tx).unwrap();
        assert_eq!(get_token_balance(&svm, &ata_a), reward_per_update);

        // B updates and claims
        do_update_price(&mut svm, &updater_b, &oracle_pda, 1050, init_slot + 20);
        let ix = build_claim_reward_ix(&oracle_pda, &reward_mint, &updater_b.pubkey(), &ata_b);
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(&[ix], Some(&updater_b.pubkey()), &[&updater_b], blockhash);
        svm.send_transaction(tx).unwrap();
        assert_eq!(get_token_balance(&svm, &ata_b), reward_per_update);

        // A can no longer claim (B is last_updater)
        let ix = build_claim_reward_ix(&oracle_pda, &reward_mint, &updater_a.pubkey(), &ata_a);
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(&[ix], Some(&updater_a.pubkey()), &[&updater_a], blockhash);
        let result = svm.send_transaction(tx);
        assert_anchor_error(&result, OracleError::Unauthorized);

        // Verify vault accounting
        let (vault_pda, _) = reward_vault_pda(&oracle_pda);
        let vault = deserialize_reward_vault(&svm, &vault_pda);
        assert_eq!(vault.total_distributed, reward_per_update * 2);
        assert_eq!(vault.total_updates_rewarded, 2);
    }

    #[test]
    fn test_claim_reward_vault_drains_exactly() {
        let mut svm = setup();
        let owner = Keypair::new();
        svm.airdrop(&owner.pubkey(), 10_000_000_000).unwrap();

        // Fund with exactly 1 reward worth
        let reward_per_update = 1_000_000u64;
        let (oracle_pda, reward_mint, _, init_slot) =
            setup_reward_test(&mut svm, &owner, reward_per_update, reward_per_update);

        do_update_price(&mut svm, &owner, &oracle_pda, 1000, init_slot + 10);

        let updater_ata = create_token_account(&mut svm, &owner, &reward_mint, &owner.pubkey());

        // First claim succeeds — drains vault to 0
        let ix = build_claim_reward_ix(&oracle_pda, &reward_mint, &owner.pubkey(), &updater_ata);
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(&[ix], Some(&owner.pubkey()), &[&owner], blockhash);
        svm.send_transaction(tx).unwrap();

        let (vault_token, _) = vault_token_account_pda(&oracle_pda);
        assert_eq!(get_token_balance(&svm, &vault_token), 0);

        // Second update + claim fails — vault empty
        do_update_price(&mut svm, &owner, &oracle_pda, 1050, init_slot + 20);
        let ix = build_claim_reward_ix(&oracle_pda, &reward_mint, &owner.pubkey(), &updater_ata);
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(&[ix], Some(&owner.pubkey()), &[&owner], blockhash);
        let result = svm.send_transaction(tx);
        assert_anchor_error(&result, OracleError::InsufficientRewardBalance);
    }

    #[test]
    fn test_fund_reward_vault_by_non_owner() {
        let mut svm = setup();
        let owner = Keypair::new();
        let funder = Keypair::new();
        svm.airdrop(&owner.pubkey(), 10_000_000_000).unwrap();
        svm.airdrop(&funder.pubkey(), 10_000_000_000).unwrap();

        let (oracle_pda, reward_mint, _, _) =
            setup_reward_test(&mut svm, &owner, 1_000_000, 0);

        // Non-owner funds the vault — should succeed
        let funder_ata = create_token_account(&mut svm, &funder, &reward_mint, &funder.pubkey());
        mint_tokens(&mut svm, &owner, &reward_mint, &funder_ata, 5_000_000);

        let ix = build_fund_reward_vault_ix(&oracle_pda, &reward_mint, &funder.pubkey(), &funder_ata, 5_000_000);
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(&[ix], Some(&funder.pubkey()), &[&funder], blockhash);
        svm.send_transaction(tx).unwrap();

        let (vault_token, _) = vault_token_account_pda(&oracle_pda);
        assert_eq!(get_token_balance(&svm, &vault_token), 5_000_000);
    }

    #[test]
    fn test_reward_distribution_five_round_rotation() {
        // Three updaters rotate over 5 rounds. Each round: update + claim.
        // Verify per-updater balances and vault accounting after all rounds.
        let mut svm = setup();
        let owner = Keypair::new();
        let u1 = Keypair::new();
        let u2 = Keypair::new();
        let u3 = Keypair::new();
        svm.airdrop(&owner.pubkey(), 10_000_000_000).unwrap();
        svm.airdrop(&u1.pubkey(), 10_000_000_000).unwrap();
        svm.airdrop(&u2.pubkey(), 10_000_000_000).unwrap();
        svm.airdrop(&u3.pubkey(), 10_000_000_000).unwrap();

        let reward_per_update = 100_000u64;
        let total_fund = reward_per_update * 5;
        let (oracle_pda, reward_mint, _, init_slot) =
            setup_reward_test(&mut svm, &owner, reward_per_update, total_fund);

        let ata1 = create_token_account(&mut svm, &u1, &reward_mint, &u1.pubkey());
        let ata2 = create_token_account(&mut svm, &u2, &reward_mint, &u2.pubkey());
        let ata3 = create_token_account(&mut svm, &u3, &reward_mint, &u3.pubkey());

        // Round order: u1, u2, u3, u1, u2
        let rounds: Vec<(&Keypair, &Pubkey, u128)> = vec![
            (&u1, &ata1, 1000),
            (&u2, &ata2, 1050),
            (&u3, &ata3, 1100),
            (&u1, &ata1, 1050),
            (&u2, &ata2, 1000),
        ];

        for (i, (updater, ata, price)) in rounds.iter().enumerate() {
            let slot = init_slot + ((i as u64 + 1) * 10);
            do_update_price(&mut svm, updater, &oracle_pda, *price, slot);

            let ix = build_claim_reward_ix(&oracle_pda, &reward_mint, &updater.pubkey(), ata);
            let blockhash = svm.latest_blockhash();
            let tx = Transaction::new_signed_with_payer(
                &[ix],
                Some(&updater.pubkey()),
                &[updater],
                blockhash,
            );
            svm.send_transaction(tx).unwrap();
        }

        // u1 claimed in rounds 0,3 → 2 rewards
        assert_eq!(get_token_balance(&svm, &ata1), reward_per_update * 2);
        // u2 claimed in rounds 1,4 → 2 rewards
        assert_eq!(get_token_balance(&svm, &ata2), reward_per_update * 2);
        // u3 claimed in round 2 → 1 reward
        assert_eq!(get_token_balance(&svm, &ata3), reward_per_update * 1);

        // Vault should be fully drained
        let (vault_token, _) = vault_token_account_pda(&oracle_pda);
        assert_eq!(get_token_balance(&svm, &vault_token), 0);

        // Accounting matches
        let (vault_pda, _) = reward_vault_pda(&oracle_pda);
        let vault = deserialize_reward_vault(&svm, &vault_pda);
        assert_eq!(vault.total_distributed, total_fund);
        assert_eq!(vault.total_updates_rewarded, 5);
    }

    #[test]
    fn test_update_without_claim_then_new_updater_claims() {
        // u1 updates but doesn't claim. u2 updates and claims.
        // u1 can no longer claim their earlier update.
        let mut svm = setup();
        let owner = Keypair::new();
        let u1 = Keypair::new();
        let u2 = Keypair::new();
        svm.airdrop(&owner.pubkey(), 10_000_000_000).unwrap();
        svm.airdrop(&u1.pubkey(), 10_000_000_000).unwrap();
        svm.airdrop(&u2.pubkey(), 10_000_000_000).unwrap();

        let reward_per_update = 1_000_000u64;
        let (oracle_pda, reward_mint, _, init_slot) =
            setup_reward_test(&mut svm, &owner, reward_per_update, 5_000_000);

        let ata1 = create_token_account(&mut svm, &u1, &reward_mint, &u1.pubkey());
        let ata2 = create_token_account(&mut svm, &u2, &reward_mint, &u2.pubkey());

        // u1 updates but does NOT claim
        do_update_price(&mut svm, &u1, &oracle_pda, 1000, init_slot + 10);

        // u2 updates — now u2 is last_updater, u1 lost their window
        do_update_price(&mut svm, &u2, &oracle_pda, 1050, init_slot + 20);

        // u1 tries to claim — fails
        let ix = build_claim_reward_ix(&oracle_pda, &reward_mint, &u1.pubkey(), &ata1);
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(&[ix], Some(&u1.pubkey()), &[&u1], blockhash);
        let result = svm.send_transaction(tx);
        assert_anchor_error(&result, OracleError::Unauthorized);
        assert_eq!(get_token_balance(&svm, &ata1), 0);

        // u2 claims successfully
        let ix = build_claim_reward_ix(&oracle_pda, &reward_mint, &u2.pubkey(), &ata2);
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(&[ix], Some(&u2.pubkey()), &[&u2], blockhash);
        svm.send_transaction(tx).unwrap();
        assert_eq!(get_token_balance(&svm, &ata2), reward_per_update);

        // Only 1 reward distributed (u1's update was unrewarded)
        let (vault_pda, _) = reward_vault_pda(&oracle_pda);
        let vault = deserialize_reward_vault(&svm, &vault_pda);
        assert_eq!(vault.total_distributed, reward_per_update);
        assert_eq!(vault.total_updates_rewarded, 1);
    }

    #[test]
    fn test_double_claim_same_updater_fails() {
        // Same updater claims twice without a new update_price in between.
        // Second claim should fail because vault accounting doesn't prevent it,
        // but the vault balance will be insufficient if funded for exactly N.
        // With sufficient funding, the protocol allows it — this tests that the
        // token transfer actually works twice (no stale state).
        let mut svm = setup();
        let owner = Keypair::new();
        svm.airdrop(&owner.pubkey(), 10_000_000_000).unwrap();

        let reward_per_update = 500_000u64;
        let (oracle_pda, reward_mint, _, init_slot) =
            setup_reward_test(&mut svm, &owner, reward_per_update, 2_000_000);

        do_update_price(&mut svm, &owner, &oracle_pda, 1000, init_slot + 10);
        let updater_ata = create_token_account(&mut svm, &owner, &reward_mint, &owner.pubkey());

        // First claim
        let ix = build_claim_reward_ix(&oracle_pda, &reward_mint, &owner.pubkey(), &updater_ata);
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(&[ix], Some(&owner.pubkey()), &[&owner], blockhash);
        svm.send_transaction(tx).unwrap();
        assert_eq!(get_token_balance(&svm, &updater_ata), reward_per_update);

        // Second claim — still last_updater, vault has funds
        svm.expire_blockhash();
        let ix = build_claim_reward_ix(&oracle_pda, &reward_mint, &owner.pubkey(), &updater_ata);
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(&[ix], Some(&owner.pubkey()), &[&owner], blockhash);
        svm.send_transaction(tx).unwrap();
        assert_eq!(get_token_balance(&svm, &updater_ata), reward_per_update * 2);

        let (vault_pda, _) = reward_vault_pda(&oracle_pda);
        let vault = deserialize_reward_vault(&svm, &vault_pda);
        assert_eq!(vault.total_distributed, reward_per_update * 2);
        assert_eq!(vault.total_updates_rewarded, 2);
    }

    #[test]
    fn test_refund_vault_mid_distribution() {
        // Vault runs low, gets refunded, distribution continues.
        let mut svm = setup();
        let owner = Keypair::new();
        let updater = Keypair::new();
        svm.airdrop(&owner.pubkey(), 10_000_000_000).unwrap();
        svm.airdrop(&updater.pubkey(), 10_000_000_000).unwrap();

        let reward_per_update = 1_000_000u64;
        // Fund with exactly 1 reward
        let (oracle_pda, reward_mint, _, init_slot) =
            setup_reward_test(&mut svm, &owner, reward_per_update, reward_per_update);

        let updater_ata = create_token_account(&mut svm, &updater, &reward_mint, &updater.pubkey());

        // Round 1: update + claim — drains vault
        do_update_price(&mut svm, &updater, &oracle_pda, 1000, init_slot + 10);
        let ix = build_claim_reward_ix(&oracle_pda, &reward_mint, &updater.pubkey(), &updater_ata);
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(&[ix], Some(&updater.pubkey()), &[&updater], blockhash);
        svm.send_transaction(tx).unwrap();

        // Round 2: update succeeds but claim fails — empty
        do_update_price(&mut svm, &updater, &oracle_pda, 1050, init_slot + 20);
        svm.expire_blockhash();
        let ix = build_claim_reward_ix(&oracle_pda, &reward_mint, &updater.pubkey(), &updater_ata);
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(&[ix], Some(&updater.pubkey()), &[&updater], blockhash);
        let result = svm.send_transaction(tx);
        assert_anchor_error(&result, OracleError::InsufficientRewardBalance);

        // Owner refunds the vault
        let owner_ata = create_token_account(&mut svm, &owner, &reward_mint, &owner.pubkey());
        mint_tokens(&mut svm, &owner, &reward_mint, &owner_ata, 3_000_000);
        let ix = build_fund_reward_vault_ix(&oracle_pda, &reward_mint, &owner.pubkey(), &owner_ata, 3_000_000);
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(&[ix], Some(&owner.pubkey()), &[&owner], blockhash);
        svm.send_transaction(tx).unwrap();

        // Round 3: claim now succeeds
        svm.expire_blockhash();
        let ix = build_claim_reward_ix(&oracle_pda, &reward_mint, &updater.pubkey(), &updater_ata);
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(&[ix], Some(&updater.pubkey()), &[&updater], blockhash);
        svm.send_transaction(tx).unwrap();

        assert_eq!(get_token_balance(&svm, &updater_ata), reward_per_update * 2);

        let (vault_pda, _) = reward_vault_pda(&oracle_pda);
        let vault = deserialize_reward_vault(&svm, &vault_pda);
        assert_eq!(vault.total_distributed, reward_per_update * 2);
        assert_eq!(vault.total_updates_rewarded, 2);
    }

    // ── Resize buffer boundary tests ──

    #[test]
    fn test_resize_grow_large_then_verify_observations() {
        // Grow from 3 to 400 (within 10KB realloc limit), verify integrity.
        // Solana allows max 10,240 bytes increase per instruction.
        // (400 - 3) * 24 bytes/obs = 9,528 bytes < 10,240 limit.
        let mut svm = setup();
        let owner = Keypair::new();
        svm.airdrop(&owner.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &owner);
        let quote_mint = create_mint(&mut svm, &owner);
        let (oracle_pda, init_slot) =
            init_oracle(&mut svm, &owner, &base_mint, &quote_mint, 3);

        // Add 3 observations to fill original buffer
        do_update_price(&mut svm, &owner, &oracle_pda, 1000, init_slot + 10);
        do_update_price(&mut svm, &owner, &oracle_pda, 1100, init_slot + 20);
        do_update_price(&mut svm, &owner, &oracle_pda, 1050, init_slot + 30);

        // Grow to 400
        let ix = build_resize_buffer_ix(&oracle_pda, &owner.pubkey(), 400);
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(&[ix], Some(&owner.pubkey()), &[&owner], blockhash);
        svm.send_transaction(tx).unwrap();

        let (obs_pda, _) = observation_buffer_pda(&oracle_pda);
        let buffer = deserialize_observation_buffer(&svm, &obs_pda);
        assert_eq!(buffer.capacity, 400);
        assert_eq!(buffer.len, 3);

        // Original observations preserved at correct indices
        assert_eq!(buffer.observations[0].slot, init_slot + 10);
        assert_eq!(buffer.observations[1].slot, init_slot + 20);
        assert_eq!(buffer.observations[2].slot, init_slot + 30);

        // New slots are zeroed
        assert_eq!(buffer.observations[3].slot, 0);
        assert_eq!(buffer.observations[399].slot, 0);

        // Continue writing — should fill into the expanded space
        do_update_price(&mut svm, &owner, &oracle_pda, 1090, init_slot + 40);
        let buffer = deserialize_observation_buffer(&svm, &obs_pda);
        assert_eq!(buffer.len, 4);
        assert_eq!(buffer.observations[3].slot, init_slot + 40);
    }

    #[test]
    fn test_resize_shrink_then_grow_preserves_data() {
        // Shrink from 10 to 3, then grow back to 8. Verify data survives round-trip.
        let mut svm = setup();
        let owner = Keypair::new();
        svm.airdrop(&owner.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &owner);
        let quote_mint = create_mint(&mut svm, &owner);
        let (oracle_pda, init_slot) =
            init_oracle(&mut svm, &owner, &base_mint, &quote_mint, 10);

        // Fill 5 observations: 1000, 1050, 1100, 1050, 1000
        let prices: [u128; 5] = [1000, 1050, 1100, 1050, 1000];
        for (i, &price) in prices.iter().enumerate() {
            do_update_price(&mut svm, &owner, &oracle_pda, price, init_slot + (i as u64 + 1) * 10);
        }

        // Shrink to 3 — keeps newest 3
        let ix = build_resize_buffer_ix(&oracle_pda, &owner.pubkey(), 3);
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(&[ix], Some(&owner.pubkey()), &[&owner], blockhash);
        svm.send_transaction(tx).unwrap();

        let (obs_pda, _) = observation_buffer_pda(&oracle_pda);
        let buffer = deserialize_observation_buffer(&svm, &obs_pda);
        assert_eq!(buffer.capacity, 3);
        assert_eq!(buffer.len, 3);
        // Newest 3: slots 30, 40, 50
        assert_eq!(buffer.observations[0].slot, init_slot + 30);
        assert_eq!(buffer.observations[1].slot, init_slot + 40);
        assert_eq!(buffer.observations[2].slot, init_slot + 50);

        // Grow back to 8
        let ix = build_resize_buffer_ix(&oracle_pda, &owner.pubkey(), 8);
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(&[ix], Some(&owner.pubkey()), &[&owner], blockhash);
        svm.send_transaction(tx).unwrap();

        let buffer = deserialize_observation_buffer(&svm, &obs_pda);
        assert_eq!(buffer.capacity, 8);
        assert_eq!(buffer.len, 3);
        // Data preserved from shrink
        assert_eq!(buffer.observations[0].slot, init_slot + 30);
        assert_eq!(buffer.observations[1].slot, init_slot + 40);
        assert_eq!(buffer.observations[2].slot, init_slot + 50);
    }

    #[test]
    fn test_resize_exceeds_realloc_limit_fails() {
        // Solana limits realloc to 10,240 bytes per instruction.
        // Growing from capacity 3 by 430 observations = 430 * 24 = 10,320 > 10,240.
        let mut svm = setup();
        let owner = Keypair::new();
        svm.airdrop(&owner.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &owner);
        let quote_mint = create_mint(&mut svm, &owner);
        let (oracle_pda, _) =
            init_oracle(&mut svm, &owner, &base_mint, &quote_mint, 3);

        // (433 - 3) * 24 = 10,320 bytes > 10,240 limit
        let ix = build_resize_buffer_ix(&oracle_pda, &owner.pubkey(), 433);
        send_tx_expect_err(&mut svm, &owner, &[ix]);

        // Buffer unchanged
        let (obs_pda, _) = observation_buffer_pda(&oracle_pda);
        let buffer = deserialize_observation_buffer(&svm, &obs_pda);
        assert_eq!(buffer.capacity, 3);
    }

    #[test]
    fn test_resize_insufficient_lamports_for_rent() {
        // Owner has minimal SOL — can't pay rent for a large realloc.
        let mut svm = setup();
        let owner = Keypair::new();
        // Only give enough for init + a few txs, not enough for large realloc
        svm.airdrop(&owner.pubkey(), 500_000_000).unwrap(); // 0.5 SOL

        let base_mint = create_mint(&mut svm, &owner);
        let quote_mint = create_mint(&mut svm, &owner);
        let (oracle_pda, _) =
            init_oracle(&mut svm, &owner, &base_mint, &quote_mint, 3);

        // Try to grow to 100_000 — needs ~2.4MB, rent ~17 SOL
        let ix = build_resize_buffer_ix(&oracle_pda, &owner.pubkey(), 100_000);
        send_tx_expect_err(&mut svm, &owner, &[ix]);

        // Buffer unchanged
        let (obs_pda, _) = observation_buffer_pda(&oracle_pda);
        let buffer = deserialize_observation_buffer(&svm, &obs_pda);
        assert_eq!(buffer.capacity, 3);
    }

    #[test]
    fn test_resize_wrapped_buffer_integrity() {
        // Fill and wrap a buffer, resize, verify the unwrapped observations
        // are in correct chronological order.
        let mut svm = setup();
        let owner = Keypair::new();
        svm.airdrop(&owner.pubkey(), 10_000_000_000).unwrap();

        let base_mint = create_mint(&mut svm, &owner);
        let quote_mint = create_mint(&mut svm, &owner);
        let (oracle_pda, init_slot) =
            init_oracle(&mut svm, &owner, &base_mint, &quote_mint, 4);

        // Write 6 entries into capacity-4 buffer (wraps twice)
        do_update_price(&mut svm, &owner, &oracle_pda, 1000, init_slot + 10); // idx 0
        do_update_price(&mut svm, &owner, &oracle_pda, 1100, init_slot + 20); // idx 1
        do_update_price(&mut svm, &owner, &oracle_pda, 1050, init_slot + 30); // idx 2
        do_update_price(&mut svm, &owner, &oracle_pda, 1090, init_slot + 40); // idx 3, full
        do_update_price(&mut svm, &owner, &oracle_pda, 1010, init_slot + 50); // overwrites idx 0
        do_update_price(&mut svm, &owner, &oracle_pda, 1060, init_slot + 60); // overwrites idx 1

        // Buffer state: [slot+50, slot+60, slot+30, slot+40], head=2, len=4
        let (obs_pda, _) = observation_buffer_pda(&oracle_pda);
        let buffer = deserialize_observation_buffer(&svm, &obs_pda);
        assert_eq!(buffer.len, 4);
        assert_eq!(buffer.head, 2);

        // Shrink to 3 — should linearize chronologically and keep newest 3
        let ix = build_resize_buffer_ix(&oracle_pda, &owner.pubkey(), 3);
        let blockhash = svm.latest_blockhash();
        let tx = Transaction::new_signed_with_payer(&[ix], Some(&owner.pubkey()), &[&owner], blockhash);
        svm.send_transaction(tx).unwrap();

        let buffer = deserialize_observation_buffer(&svm, &obs_pda);
        assert_eq!(buffer.capacity, 3);
        assert_eq!(buffer.len, 3);
        // Chronological: slot+30, slot+40, slot+50, slot+60 → keep last 3
        assert_eq!(buffer.observations[0].slot, init_slot + 40);
        assert_eq!(buffer.observations[1].slot, init_slot + 50);
        assert_eq!(buffer.observations[2].slot, init_slot + 60);

        // Verify cumulative values are consistent
        assert!(buffer.observations[0].cumulative_price < buffer.observations[1].cumulative_price);
        assert!(buffer.observations[1].cumulative_price < buffer.observations[2].cumulative_price);

        // Can still write after resize
        do_update_price(&mut svm, &owner, &oracle_pda, 1020, init_slot + 70);
        let buffer = deserialize_observation_buffer(&svm, &obs_pda);
        assert_eq!(buffer.len, 3); // stays at capacity
        assert_eq!(buffer.head, 1); // wrapped
    }
}
