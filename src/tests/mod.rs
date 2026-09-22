#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use litesvm::LiteSVM;
    use litesvm_token::{spl_token::{self}, CreateAssociatedTokenAccount, CreateMint, MintTo};
    use solana_instruction::{AccountMeta, Instruction};
    use solana_keypair::Keypair;
    use solana_message::Message;
    use solana_native_token::LAMPORTS_PER_SOL;
    use solana_pubkey::Pubkey;
    use solana_signer::Signer;
    use solana_transaction::Transaction;
    use solana_program_pack::Pack;

    const PROGRAM_ID: &str = "4ibrEMW5F6hKnkW4jVedswYv6H6VtwPN6ar6dvXDN1nT";
    const TOKEN_PROGRAM_ID: Pubkey = spl_token::ID;
    const ASSOCIATED_TOKEN_PROGRAM_ID: &str = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL";

    fn program_id() -> Pubkey {
        Pubkey::from(crate::ID)
    }

    fn setup() -> (LiteSVM, Keypair) {
        let mut svm = LiteSVM::new();
        let payer = Keypair::new();

        #[allow(deprecated)]
        svm.set_sysvar(&solana_rent::Rent {
            lamports_per_byte_year: 6960,
            exemption_threshold: 1.0,
            burn_percent: 50,
        });

        svm.airdrop(&payer.pubkey(), 10 * LAMPORTS_PER_SOL)
            .expect("Airdrop failed");

        let so_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/deploy/escrow.so");

        let program_data = std::fs::read(&so_path).unwrap_or_else(|e| {
            panic!(
                "Failed to read program SO file at {}: {e}. Run `cargo build-sbf` first.",
                so_path.display()
            )
        });

        svm.add_program(program_id(), &program_data)
            .expect("Failed to add program");

        (svm, payer)
    }

    struct MakeResult {
        svm: LiteSVM,
        maker: Keypair,
        mint_a: Pubkey,
        mint_b: Pubkey,
        escrow: Pubkey,
        bump: u8,
        vault: Pubkey,
        maker_ata_a: Pubkey,
        amount_to_receive: u64,
        amount_to_give: u64,
    }

    fn run_make() -> MakeResult {
        let (mut svm, maker) = setup();

        let mint_a = CreateMint::new(&mut svm, &maker)
            .decimals(6)
            .authority(&maker.pubkey())
            .send()
            .unwrap();

        let mint_b = CreateMint::new(&mut svm, &maker)
            .decimals(6)
            .authority(&maker.pubkey())
            .send()
            .unwrap();

        let maker_ata_a = CreateAssociatedTokenAccount::new(&mut svm, &maker, &mint_a)
            .owner(&maker.pubkey())
            .send()
            .unwrap();

        let (escrow, bump) = Pubkey::find_program_address(
            &[b"escrow", maker.pubkey().as_ref()],
            &program_id(),
        );
        let vault = spl_associated_token_account::get_associated_token_address(&escrow, &mint_a);

        MintTo::new(&mut svm, &maker, &mint_a, &maker_ata_a, 1_000_000_000)
            .send()
            .unwrap();

        let amount_to_receive = 100_000_000u64;
        let amount_to_give = 500_000_000u64;

        let make_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(maker.pubkey(), true),
                AccountMeta::new(mint_a, false),
                AccountMeta::new(mint_b, false),
                AccountMeta::new(escrow, false),
                AccountMeta::new(maker_ata_a, false),
                AccountMeta::new(vault, false),
                AccountMeta::new(solana_sdk_ids::system_program::ID, false),
                AccountMeta::new(TOKEN_PROGRAM_ID, false),
                AccountMeta::new(ASSOCIATED_TOKEN_PROGRAM_ID.parse().unwrap(), false),
            ],
            data: [
                vec![0u8],
                amount_to_receive.to_le_bytes().to_vec(),
                amount_to_give.to_le_bytes().to_vec(),
            ]
            .concat(),
        };

        let message = Message::new(&[make_ix], Some(&maker.pubkey()));
        let tx = Transaction::new(&[&maker], message, svm.latest_blockhash());
        let result = svm.send_transaction(tx).unwrap();
        println!("Make CUs Consumed: {}", result.compute_units_consumed);

        MakeResult {
            svm,
            maker,
            mint_a,
            mint_b,
            escrow,
            bump,
            vault,
            maker_ata_a,
            amount_to_receive,
            amount_to_give,
        }
    }

    fn token_balance(svm: &LiteSVM, account: &Pubkey) -> u64 {
        let data = svm
            .get_account(account)
            .expect("token account must exist");
        spl_token_2022::state::Account::unpack(&data.data)
            .expect("valid token account")
            .amount
    }

    fn assert_closed(svm: &LiteSVM, account: &Pubkey) {
        if let Some(acc) = svm.get_account(account) {
            assert_eq!(acc.lamports, 0, "closed account retained lamports");
            assert_eq!(
                acc.owner,
                solana_sdk_ids::system_program::ID,
                "closed account retained its old owner"
            );
            assert_eq!(acc.data.len(), 0, "closed account retained data");
        }
    }

    #[test]
    fn test_make_instruction() {
        let r = run_make();

        let vault = r.svm.get_account(&r.vault).unwrap();
        let vault_state = spl_token_2022::state::Account::unpack(&vault.data).unwrap();
        assert_eq!(vault_state.owner, r.escrow);
        assert_eq!(vault_state.mint, r.mint_a);
        assert_eq!(vault_state.amount, r.amount_to_give);
        assert_eq!(
            token_balance(&r.svm, &r.maker_ata_a),
            1_000_000_000 - r.amount_to_give
        );

        let escrow = r.svm.get_account(&r.escrow).unwrap();
        assert_eq!(escrow.owner, program_id());
        assert_eq!(escrow.data.len(), 113);
        assert_eq!(&escrow.data[0..32], r.maker.pubkey().as_ref());
        assert_eq!(&escrow.data[32..64], r.mint_a.as_ref());
        assert_eq!(&escrow.data[64..96], r.mint_b.as_ref());
        assert_eq!(
            u64::from_le_bytes(escrow.data[96..104].try_into().unwrap()),
            r.amount_to_receive
        );
        assert_eq!(
            u64::from_le_bytes(escrow.data[104..112].try_into().unwrap()),
            r.amount_to_give
        );
        assert_eq!(escrow.data[112], r.bump);
    }

    #[test]
    fn test_take_instruction() {
        let mut r = run_make();

        let taker = Keypair::new();
        r.svm
            .airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL)
            .unwrap();

        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut r.svm, &taker, &r.mint_b)
            .owner(&taker.pubkey())
            .send()
            .unwrap();
        MintTo::new(
            &mut r.svm,
            &r.maker,
            &r.mint_b,
            &taker_ata_b,
            r.amount_to_receive,
        )
        .send()
        .unwrap();

        let taker_ata_a =
            spl_associated_token_account::get_associated_token_address(&taker.pubkey(), &r.mint_a);
        let maker_ata_b =
            spl_associated_token_account::get_associated_token_address(&r.maker.pubkey(), &r.mint_b);

        let take_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(taker.pubkey(), true),
                AccountMeta::new(r.maker.pubkey(), false),
                AccountMeta::new_readonly(r.mint_a, false),
                AccountMeta::new_readonly(r.mint_b, false),
                AccountMeta::new(r.escrow, false),
                AccountMeta::new(r.vault, false),
                AccountMeta::new(taker_ata_a, false),
                AccountMeta::new(taker_ata_b, false),
                AccountMeta::new(maker_ata_b, false),
                AccountMeta::new_readonly(solana_sdk_ids::system_program::ID, false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
                AccountMeta::new_readonly(
                    ASSOCIATED_TOKEN_PROGRAM_ID.parse().unwrap(),
                    false,
                ),
            ],
            data: vec![1u8],
        };

        let message = Message::new(&[take_ix], Some(&taker.pubkey()));
        let tx = Transaction::new(&[&taker], message, r.svm.latest_blockhash());
        let result = r.svm.send_transaction(tx).unwrap();
        println!("Take CUs Consumed: {}", result.compute_units_consumed);

        assert_eq!(token_balance(&r.svm, &taker_ata_a), r.amount_to_give);
        assert_eq!(token_balance(&r.svm, &maker_ata_b), r.amount_to_receive);
        assert_eq!(token_balance(&r.svm, &taker_ata_b), 0);
        assert_closed(&r.svm, &r.vault);
        assert_closed(&r.svm, &r.escrow);
    }

    #[test]
    fn test_cancel_instruction() {
        let mut r = run_make();

        let cancel_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(r.maker.pubkey(), true),
                AccountMeta::new_readonly(r.mint_a, false),
                AccountMeta::new(r.escrow, false),
                AccountMeta::new(r.vault, false),
                AccountMeta::new(r.maker_ata_a, false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            ],
            data: vec![2u8],
        };

        let message = Message::new(&[cancel_ix], Some(&r.maker.pubkey()));
        let tx = Transaction::new(&[&r.maker], message, r.svm.latest_blockhash());
        let result = r.svm.send_transaction(tx).unwrap();
        println!("Cancel CUs Consumed: {}", result.compute_units_consumed);

        assert_eq!(token_balance(&r.svm, &r.maker_ata_a), 1_000_000_000);
        assert_closed(&r.svm, &r.vault);
        assert_closed(&r.svm, &r.escrow);
    }

    #[test]
    fn test_take_fails_when_taker_underfunded() {
        let mut r = run_make();

        let taker = Keypair::new();
        r.svm
            .airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL)
            .unwrap();

        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut r.svm, &taker, &r.mint_b)
            .owner(&taker.pubkey())
            .send()
            .unwrap();
        MintTo::new(&mut r.svm, &r.maker, &r.mint_b, &taker_ata_b, 50_000_000)
            .send()
            .unwrap();

        let taker_ata_a =
            spl_associated_token_account::get_associated_token_address(&taker.pubkey(), &r.mint_a);
        let maker_ata_b =
            spl_associated_token_account::get_associated_token_address(&r.maker.pubkey(), &r.mint_b);

        let take_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(taker.pubkey(), true),
                AccountMeta::new(r.maker.pubkey(), false),
                AccountMeta::new_readonly(r.mint_a, false),
                AccountMeta::new_readonly(r.mint_b, false),
                AccountMeta::new(r.escrow, false),
                AccountMeta::new(r.vault, false),
                AccountMeta::new(taker_ata_a, false),
                AccountMeta::new(taker_ata_b, false),
                AccountMeta::new(maker_ata_b, false),
                AccountMeta::new_readonly(solana_sdk_ids::system_program::ID, false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
                AccountMeta::new_readonly(
                    ASSOCIATED_TOKEN_PROGRAM_ID.parse().unwrap(),
                    false,
                ),
            ],
            data: vec![1u8],
        };

        let message = Message::new(&[take_ix], Some(&taker.pubkey()));
        let tx = Transaction::new(&[&taker], message, r.svm.latest_blockhash());
        let result = r.svm.send_transaction(tx);
        println!("Underfunded Take failed: {}", result.is_err());
        assert!(result.is_err());

        assert_eq!(token_balance(&r.svm, &r.vault), r.amount_to_give);
        assert_eq!(
            token_balance(&r.svm, &r.maker_ata_a),
            1_000_000_000 - r.amount_to_give
        );
    }

    #[test]
    fn test_cancel_by_stranger() {
        let mut r = run_make();

        let stranger = Keypair::new();
        r.svm
            .airdrop(&stranger.pubkey(), 10 * LAMPORTS_PER_SOL)
            .unwrap();

        let cancel_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(stranger.pubkey(), true),
                AccountMeta::new_readonly(r.mint_a, false),
                AccountMeta::new(r.escrow, false),
                AccountMeta::new(r.vault, false),
                AccountMeta::new(r.maker_ata_a, false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            ],
            data: vec![2u8],
        };

        let message = Message::new(&[cancel_ix], Some(&stranger.pubkey()));
        let tx = Transaction::new(&[&stranger], message, r.svm.latest_blockhash());
        let result = r.svm.send_transaction(tx);
        println!("Stranger Cancel failed: {}", result.is_err());
        assert!(result.is_err());

        assert_eq!(token_balance(&r.svm, &r.vault), r.amount_to_give);
        assert!(r.svm.get_account(&r.escrow).is_some());
    }

    #[test]
    fn test_take_with_mismatched_maker() {
        let mut r = run_make();

        let taker = Keypair::new();
        let fake_maker = Keypair::new();
        r.svm
            .airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL)
            .unwrap();

        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut r.svm, &taker, &r.mint_b)
            .owner(&taker.pubkey())
            .send()
            .unwrap();
        MintTo::new(
            &mut r.svm,
            &r.maker,
            &r.mint_b,
            &taker_ata_b,
            r.amount_to_receive,
        )
        .send()
        .unwrap();

        let taker_ata_a =
            spl_associated_token_account::get_associated_token_address(&taker.pubkey(), &r.mint_a);
        let fake_maker_ata_b = spl_associated_token_account::get_associated_token_address(
            &fake_maker.pubkey(),
            &r.mint_b,
        );

        let take_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(taker.pubkey(), true),
                AccountMeta::new(fake_maker.pubkey(), false),
                AccountMeta::new_readonly(r.mint_a, false),
                AccountMeta::new_readonly(r.mint_b, false),
                AccountMeta::new(r.escrow, false),
                AccountMeta::new(r.vault, false),
                AccountMeta::new(taker_ata_a, false),
                AccountMeta::new(taker_ata_b, false),
                AccountMeta::new(fake_maker_ata_b, false),
                AccountMeta::new_readonly(solana_sdk_ids::system_program::ID, false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
                AccountMeta::new_readonly(
                    ASSOCIATED_TOKEN_PROGRAM_ID.parse().unwrap(),
                    false,
                ),
            ],
            data: vec![1u8],
        };

        let message = Message::new(&[take_ix], Some(&taker.pubkey()));
        let tx = Transaction::new(&[&taker], message, r.svm.latest_blockhash());
        let result = r.svm.send_transaction(tx);
        println!("Mismatched-maker Take failed: {}", result.is_err());
        assert!(result.is_err());

        assert_eq!(token_balance(&r.svm, &r.vault), r.amount_to_give);
        assert!(r.svm.get_account(&r.escrow).is_some());
    }
}
