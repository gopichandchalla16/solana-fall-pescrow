use pinocchio::{AccountView, ProgramResult, cpi::{Seed, Signer}, error::ProgramError};
use crate::state::Escrow;

/// Account order: taker, maker, mint_a, mint_b, escrow_account, vault,
/// taker_ata_a, taker_ata_b, maker_ata_b, system_program, token_program,
/// associated_token_program.
pub fn process_take_instruction(accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    if !data.is_empty() { return Err(ProgramError::InvalidInstructionData); }

    let [taker, maker, mint_a, mint_b, escrow_account, vault, taker_ata_a,
        taker_ata_b, maker_ata_b, system_program, token_program,
        _associated_token_program] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    if !taker.is_signer() { return Err(ProgramError::MissingRequiredSignature); }
    if !escrow_account.owned_by(&crate::ID) { return Err(ProgramError::IllegalOwner); }

    let (amount_to_receive, bump) = {
        let escrow = Escrow::load_mut(escrow_account)?;
        if escrow.maker() != *maker.address()
            || escrow.mint_a() != *mint_a.address()
            || escrow.mint_b() != *mint_b.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        (escrow.amount_to_receive(), escrow.bump)
    };

    let expected_escrow = pinocchio_pubkey::derive_address(
        &[b"escrow", maker.address().as_ref(), &[bump]],
        None,
        &crate::ID.to_bytes(),
    );
    if expected_escrow != *escrow_account.address() {
        return Err(ProgramError::InvalidSeeds);
    }

    let vault_amount = {
        let s = pinocchio_token::state::Account::from_account_view(vault)?;
        if s.owner() != escrow_account.address() || s.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        s.amount()
    };

    {
        let s = pinocchio_token::state::Account::from_account_view(taker_ata_b)?;
        if s.owner() != taker.address() || s.mint() != mint_b.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    pinocchio_associated_token_account::instructions::CreateIdempotent {
        funding_account: taker, account: taker_ata_a, wallet: taker, mint: mint_a,
        token_program, system_program,
    }.invoke()?;

    pinocchio_associated_token_account::instructions::CreateIdempotent {
        funding_account: taker, account: maker_ata_b, wallet: maker, mint: mint_b,
        token_program, system_program,
    }.invoke()?;

    let bump_bytes = [bump];
    let seeds = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_array()),
        Seed::from(&bump_bytes),
    ];
    let signer = Signer::from(&seeds);

    pinocchio_token::instructions::Transfer {
        from: taker_ata_b, to: maker_ata_b, authority: taker,
        multisig_signers: &[] as &[&AccountView], amount: amount_to_receive,
    }.invoke()?;

    pinocchio_token::instructions::Transfer {
        from: vault, to: taker_ata_a, authority: escrow_account,
        multisig_signers: &[] as &[&AccountView], amount: vault_amount,
    }.invoke_signed(&[signer.clone()])?;

    pinocchio_token::instructions::CloseAccount {
        account: vault, destination: maker, authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }.invoke_signed(&[signer])?;

    let lamports = escrow_account.lamports();
    maker.set_lamports(maker.lamports() + lamports);
    escrow_account.set_lamports(0);
    escrow_account.close()?;
    Ok(())
}
