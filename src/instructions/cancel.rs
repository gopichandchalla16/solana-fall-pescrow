use pinocchio::{AccountView, ProgramResult, cpi::{Seed, Signer}, error::ProgramError};
use crate::state::Escrow;

/// Account order: maker, mint_a, escrow_account, vault, maker_ata_a, token_program.
pub fn process_cancel_instruction(accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    if !data.is_empty() { return Err(ProgramError::InvalidInstructionData); }

    let [maker, mint_a, escrow_account, vault, maker_ata_a, _token_program] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    if !maker.is_signer() { return Err(ProgramError::MissingRequiredSignature); }
    if !escrow_account.owned_by(&crate::ID) { return Err(ProgramError::IllegalOwner); }

    let bump = {
        let escrow = Escrow::load_mut(escrow_account)?;
        if escrow.maker() != *maker.address() || escrow.mint_a() != *mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        escrow.bump
    };

    let expected_escrow = pinocchio_pubkey::derive_address(
        &[b"escrow", maker.address().as_ref(), &[bump]],
        None,
        &crate::ID.to_bytes(),
    );
    if expected_escrow != *escrow_account.address().as_array() {
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
        let s = pinocchio_token::state::Account::from_account_view(maker_ata_a)?;
        if s.owner() != maker.address() || s.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    let bump_bytes = [bump];
    let seeds = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_array()),
        Seed::from(&bump_bytes),
    ];
    let signer = Signer::from(&seeds);

    pinocchio_token::instructions::Transfer {
        from: vault, to: maker_ata_a, authority: escrow_account,
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
