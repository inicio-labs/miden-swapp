use integration::helpers::{
    build_project_in_dir, create_testing_account_from_package, AccountCreationConfig,
};

use miden_client::{
    note::{Note, NoteAssets, NoteMetadata, NoteTag, NoteType},
    transaction::OutputNote,
    Felt, Word,
};
use miden_core::{crypto::hash::Rpo256, FieldElement};
use miden_protocol::{
    account::AccountId,
    asset::{Asset, FungibleAsset},
    note::{NoteAttachment, NoteAttachmentScheme},
    transaction::TransactionScript,
};
use miden_standards::note::utils::build_p2id_recipient;
use miden_testing::{Auth, MockChain};
use std::{path::Path, sync::Arc};

/// Compute the P2ID tag for a local account
fn compute_p2id_tag_for_local_account(account_id: AccountId) -> NoteTag {
    NoteTag::with_account_target(account_id)
}

#[tokio::test]
async fn p2id_tx_script_test() -> anyhow::Result<()> {
    println!("=== Test: P2ID Transaction Script ===");
    println!("Alice creates a P2ID note for Bob using the p2id-tx-script");

    let mut builder = MockChain::builder();

    // STEP 1: Create a USDC faucet
    println!("Creating USDC faucet...");
    let usdc_faucet = builder.add_existing_basic_faucet(
        Auth::BasicAuth,
        "USDC",
        1000,      // max_supply
        Some(100), // total_issuance
    )?;
    println!("USDC Faucet: {:?}", usdc_faucet.id());

    // STEP 2: Create Alice's account (the tx executor) using basic-wallet
    println!("\nBuilding basic-wallet contract...");
    let account_package = Arc::new(build_project_in_dir(
        Path::new("../contracts/basic-wallet"),
        true,
    )?);
    println!("Basic-wallet contract built successfully.");

    let alice_cfg = AccountCreationConfig {
        storage_slots: vec![],
        ..Default::default()
    };

    // Alice has no assets — the p2id-tx-script adds assets inflight (not from vault)
    let alice =
        create_testing_account_from_package(account_package.clone(), alice_cfg, vec![]).await?;
    println!("Alice account created: {:?}", alice.id());

    builder.add_account(alice.clone())?;

    // STEP 3: Create Bob's account (the P2ID recipient)
    println!("\nCreating Bob wallet...");
    let bob = builder.add_existing_wallet_with_assets(Auth::BasicAuth, [])?;
    println!("Bob: {:?}", bob.id());

    // STEP 4: Build MockChain
    println!("\nBuilding MockChain...");
    let mock_chain = builder.build()?;

    // STEP 5: Build p2id-tx-script
    println!("\nBuilding p2id-tx-script contract...");
    let p2id_script_package = Arc::new(build_project_in_dir(
        Path::new("../contracts/p2id-tx-script"),
        true,
    )?);
    println!("p2id-tx-script contract built successfully.");

    let program = p2id_script_package.unwrap_program();
    let tx_script =
        TransactionScript::from_parts(program.mast_forest().clone(), program.entrypoint());

    // STEP 6: Prepare P2ID parameters
    let amount = 50u64;
    let serial_num = Word::from([Felt::new(1), Felt::new(2), Felt::new(3), Felt::new(4)]);
    let asset = FungibleAsset::new(usdc_faucet.id(), amount)?;
    let asset_word = Word::from(Asset::from(asset));
    let tag = compute_p2id_tag_for_local_account(bob.id());
    let tag_felt = Felt::new(u32::from(tag) as u64);
    let note_type = NoteType::Public;
    let note_type_felt: Felt = note_type.into();
    let aux = Felt::new(amount);

    println!("P2ID parameters:");
    println!("  Recipient (Bob): {:?}", bob.id());
    println!("  Asset: {} USDC", amount);
    println!("  Tag: {:?}", tag.as_u32());
    println!("  Serial num: {:?}", serial_num);

    // STEP 7: Build advice stack
    // The tx-script reads 4 words from the advice stack via adv_load_preimage.
    // AdviceInputs.stack[0] is the first element popped (top of stack).
    //
    // Layout (4 words = 16 felts, top → bottom):
    //   Word 0: serial_num [s0, s1, s2, s3]
    //   Word 1: [recipient_prefix, recipient_suffix, tag, note_type]
    //   Word 2: [aux, 0, 0, 0]
    //   Word 3: asset_word [a0, a1, a2, a3]
    let advice_stack: Vec<Felt> = vec![
        // Word 0: serial_num
        serial_num[0],
        serial_num[1],
        serial_num[2],
        serial_num[3],
        // Word 1: [recipient_prefix, recipient_suffix, tag, note_type]
        bob.id().prefix().into(),
        bob.id().suffix(),
        tag_felt,
        note_type_felt,
        // Word 2: [aux, 0, 0, 0]
        aux,
        Felt::ZERO,
        Felt::ZERO,
        Felt::ZERO,
        // Word 3: asset_word
        asset_word[0],
        asset_word[1],
        asset_word[2],
        asset_word[3],
    ];

    // STEP 8: Compute RPO hash of advice data (needed by adv_load_preimage)
    let commitment_key: Word = Rpo256::hash_elements(&advice_stack);

    let mut commitment = commitment_key.clone();
    commitment.reverse();

    // STEP 9: Create expected P2ID output note
    println!("\nCreating expected P2ID output note...");
    let recipient = build_p2id_recipient(bob.id(), serial_num)?;
    let note_assets = NoteAssets::new(vec![asset.into()])?;

    let aux_word = Word::from([aux, Felt::ZERO, Felt::ZERO, Felt::ZERO]);
    let attachment = NoteAttachment::new_word(NoteAttachmentScheme::none(), aux_word);

    let note_metadata =
        NoteMetadata::new(alice.id(), note_type, tag).with_attachment(attachment.clone());
    let expected_p2id_note = Note::new(note_assets, note_metadata, recipient);
    println!("Expected P2ID note: {:?}", expected_p2id_note.id());

    // STEP 10: Execute transaction
    println!("\nExecuting transaction with p2id-tx-script...");
    let tx_context = mock_chain
        .build_tx_context(alice.id(), &[], &[])?
        .tx_script(tx_script)
        .tx_script_args(commitment)
        .extend_advice_map([(commitment_key, advice_stack)])
        .extend_expected_output_notes(vec![OutputNote::Full(expected_p2id_note.clone())])
        .build()?;

    let executed_transaction = tx_context.execute().await?;

    println!(
        "Transaction executed. Cycle count: {:?}",
        executed_transaction.measurements().tx_script_processing
    );

    // STEP 11: Verify results
    println!("\n=== Verification ===");

    // Check output notes — should have exactly 1 P2ID note
    let output_notes = executed_transaction.output_notes();
    println!("Output notes created: {}", output_notes.num_notes());
    assert_eq!(output_notes.num_notes(), 1, "Expected exactly 1 P2ID note");

    // Verify attachment
    assert_eq!(
        output_notes.get_note(0).metadata().attachment().clone(),
        attachment,
        "P2ID note attachment should match"
    );

    let p2id_note = output_notes.get_note(0);
    println!("P2ID note created: {:?}", p2id_note.id());

    // Verify P2ID note contains the correct asset
    let p2id_assets = p2id_note.assets().unwrap();
    assert_eq!(
        p2id_assets.num_assets(),
        1,
        "P2ID note should have exactly 1 asset"
    );

    let p2id_asset = p2id_assets.iter().next().unwrap();
    let p2id_fungible = match p2id_asset {
        Asset::Fungible(f) => f,
        _ => panic!("Expected fungible asset in P2ID note"),
    };
    assert_eq!(
        p2id_fungible.faucet_id(),
        usdc_faucet.id(),
        "P2ID note should contain USDC"
    );
    assert_eq!(
        p2id_fungible.amount(),
        amount,
        "P2ID note should contain {} USDC",
        amount
    );
    println!("P2ID note verified: {} USDC for Bob", amount);

    // Verify no account vault changes (asset was added inflight, not from vault)
    let account_delta = executed_transaction.account_delta();
    let vault_delta = account_delta.vault();
    let added_assets: Vec<Asset> = vault_delta.added_assets().collect();
    let removed_assets: Vec<Asset> = vault_delta.removed_assets().collect();

    assert_eq!(added_assets.len(), 0, "Alice should not receive any assets");
    assert_eq!(
        removed_assets.len(),
        0,
        "Alice should not lose any assets (inflight only)"
    );
    println!("Alice's vault unchanged — asset added inflight only");

    println!("\nP2ID tx-script test passed!");
    println!("  - P2ID note created for Bob with {} USDC", amount);
    println!("  - No account vault changes (inflight asset)");

    Ok(())
}

#[tokio::test]
async fn p2id_tx_script_multi_note_test() -> anyhow::Result<()> {
    println!("=== Test: P2ID Transaction Script — Multi-Note ===");
    println!("Alice creates 2 P2ID notes (USDC→Bob, ETH→Charlie) in one tx");

    let mut builder = MockChain::builder();

    // STEP 1: Create faucets
    println!("Creating faucets...");
    let usdc_faucet = builder.add_existing_basic_faucet(
        Auth::BasicAuth,
        "USDC",
        1000,
        Some(100),
    )?;
    let eth_faucet = builder.add_existing_basic_faucet(
        Auth::BasicAuth,
        "ETH",
        1000,
        Some(100),
    )?;
    println!("USDC Faucet: {:?}", usdc_faucet.id());
    println!("ETH Faucet: {:?}", eth_faucet.id());

    // STEP 2: Create Alice's account (the tx executor)
    println!("\nBuilding basic-wallet contract...");
    let account_package = Arc::new(build_project_in_dir(
        Path::new("../contracts/basic-wallet"),
        true,
    )?);

    let alice_cfg = AccountCreationConfig {
        storage_slots: vec![],
        ..Default::default()
    };
    let alice =
        create_testing_account_from_package(account_package.clone(), alice_cfg, vec![]).await?;
    println!("Alice account created: {:?}", alice.id());
    builder.add_account(alice.clone())?;

    // STEP 3: Create Bob and Charlie (recipients)
    let bob = builder.add_existing_wallet_with_assets(Auth::BasicAuth, [])?;
    let charlie = builder.add_existing_wallet_with_assets(Auth::BasicAuth, [])?;
    println!("Bob: {:?}", bob.id());
    println!("Charlie: {:?}", charlie.id());

    // STEP 4: Build MockChain
    let mock_chain = builder.build()?;

    // STEP 5: Build p2id-tx-script
    println!("\nBuilding p2id-tx-script contract...");
    let p2id_script_package = Arc::new(build_project_in_dir(
        Path::new("../contracts/p2id-tx-script"),
        true,
    )?);
    let program = p2id_script_package.unwrap_program();
    let tx_script =
        TransactionScript::from_parts(program.mast_forest().clone(), program.entrypoint());

    // STEP 6: Prepare Note 1 parameters (50 USDC → Bob)
    let amount1 = 50u64;
    let serial_num1 = Word::from([Felt::new(1), Felt::new(2), Felt::new(3), Felt::new(4)]);
    let asset1 = FungibleAsset::new(usdc_faucet.id(), amount1)?;
    let asset_word1 = Word::from(Asset::from(asset1));
    let tag1 = compute_p2id_tag_for_local_account(bob.id());
    let tag_felt1 = Felt::new(u32::from(tag1) as u64);
    let note_type1 = NoteType::Public;
    let note_type_felt1: Felt = note_type1.into();
    let aux1 = Felt::new(amount1);

    // STEP 7: Prepare Note 2 parameters (30 ETH → Charlie)
    let amount2 = 30u64;
    let serial_num2 = Word::from([Felt::new(5), Felt::new(6), Felt::new(7), Felt::new(8)]);
    let asset2 = FungibleAsset::new(eth_faucet.id(), amount2)?;
    let asset_word2 = Word::from(Asset::from(asset2));
    let tag2 = compute_p2id_tag_for_local_account(charlie.id());
    let tag_felt2 = Felt::new(u32::from(tag2) as u64);
    let note_type2 = NoteType::Public;
    let note_type_felt2: Felt = note_type2.into();
    let aux2 = Felt::new(amount2);

    // STEP 8: Build advice stack — 32 felts (2 notes × 16 felts)
    let advice_stack: Vec<Felt> = vec![
        // --- Note 1 (USDC → Bob) ---
        // Word 0: serial_num
        serial_num1[0], serial_num1[1], serial_num1[2], serial_num1[3],
        // Word 1: [recipient_prefix, recipient_suffix, tag, note_type]
        bob.id().prefix().into(), bob.id().suffix(), tag_felt1, note_type_felt1,
        // Word 2: [aux, 0, 0, 0]
        aux1, Felt::ZERO, Felt::ZERO, Felt::ZERO,
        // Word 3: asset_word
        asset_word1[0], asset_word1[1], asset_word1[2], asset_word1[3],
        // --- Note 2 (ETH → Charlie) ---
        // Word 0: serial_num
        serial_num2[0], serial_num2[1], serial_num2[2], serial_num2[3],
        // Word 1: [recipient_prefix, recipient_suffix, tag, note_type]
        charlie.id().prefix().into(), charlie.id().suffix(), tag_felt2, note_type_felt2,
        // Word 2: [aux, 0, 0, 0]
        aux2, Felt::ZERO, Felt::ZERO, Felt::ZERO,
        // Word 3: asset_word
        asset_word2[0], asset_word2[1], asset_word2[2], asset_word2[3],
    ];

    // STEP 9: Compute RPO hash commitment
    let commitment_key: Word = Rpo256::hash_elements(&advice_stack);
    let mut commitment = commitment_key.clone();
    commitment.reverse();

    // STEP 10: Create expected output notes
    let recipient1 = build_p2id_recipient(bob.id(), serial_num1)?;
    let note_assets1 = NoteAssets::new(vec![asset1.into()])?;
    let aux_word1 = Word::from([aux1, Felt::ZERO, Felt::ZERO, Felt::ZERO]);
    let attachment1 = NoteAttachment::new_word(NoteAttachmentScheme::none(), aux_word1);
    let note_metadata1 =
        NoteMetadata::new(alice.id(), note_type1, tag1).with_attachment(attachment1.clone());
    let expected_note1 = Note::new(note_assets1, note_metadata1, recipient1);

    let recipient2 = build_p2id_recipient(charlie.id(), serial_num2)?;
    let note_assets2 = NoteAssets::new(vec![asset2.into()])?;
    let aux_word2 = Word::from([aux2, Felt::ZERO, Felt::ZERO, Felt::ZERO]);
    let attachment2 = NoteAttachment::new_word(NoteAttachmentScheme::none(), aux_word2);
    let note_metadata2 =
        NoteMetadata::new(alice.id(), note_type2, tag2).with_attachment(attachment2.clone());
    let expected_note2 = Note::new(note_assets2, note_metadata2, recipient2);

    println!("Expected note 1: {:?}", expected_note1.id());
    println!("Expected note 2: {:?}", expected_note2.id());

    // STEP 11: Execute transaction
    println!("\nExecuting transaction with p2id-tx-script (2 notes)...");
    let tx_context = mock_chain
        .build_tx_context(alice.id(), &[], &[])?
        .tx_script(tx_script)
        .tx_script_args(commitment)
        .extend_advice_map([(commitment_key, advice_stack)])
        .extend_expected_output_notes(vec![
            OutputNote::Full(expected_note1.clone()),
            OutputNote::Full(expected_note2.clone()),
        ])
        .build()?;

    let executed_transaction = tx_context.execute().await?;

    println!(
        "Transaction executed. Cycle count: {:?}",
        executed_transaction.measurements().tx_script_processing
    );

    // STEP 12: Verify results
    println!("\n=== Verification ===");
    let output_notes = executed_transaction.output_notes();
    println!("Output notes created: {}", output_notes.num_notes());
    assert_eq!(output_notes.num_notes(), 2, "Expected exactly 2 P2ID notes");

    // Verify note 0 (USDC → Bob)
    let note0 = output_notes.get_note(0);
    assert_eq!(
        note0.metadata().attachment().clone(),
        attachment1,
        "Note 0 attachment should match"
    );
    let assets0 = note0.assets().unwrap();
    assert_eq!(assets0.num_assets(), 1);
    let fungible0 = match assets0.iter().next().unwrap() {
        Asset::Fungible(f) => f,
        _ => panic!("Expected fungible asset in note 0"),
    };
    assert_eq!(fungible0.faucet_id(), usdc_faucet.id());
    assert_eq!(fungible0.amount(), amount1);
    println!("Note 0 verified: {} USDC for Bob", amount1);

    // Verify note 1 (ETH → Charlie)
    let note1 = output_notes.get_note(1);
    assert_eq!(
        note1.metadata().attachment().clone(),
        attachment2,
        "Note 1 attachment should match"
    );
    let assets1 = note1.assets().unwrap();
    assert_eq!(assets1.num_assets(), 1);
    let fungible1 = match assets1.iter().next().unwrap() {
        Asset::Fungible(f) => f,
        _ => panic!("Expected fungible asset in note 1"),
    };
    assert_eq!(fungible1.faucet_id(), eth_faucet.id());
    assert_eq!(fungible1.amount(), amount2);
    println!("Note 1 verified: {} ETH for Charlie", amount2);

    // Verify no vault changes
    let account_delta = executed_transaction.account_delta();
    let vault_delta = account_delta.vault();
    let added_assets: Vec<Asset> = vault_delta.added_assets().collect();
    let removed_assets: Vec<Asset> = vault_delta.removed_assets().collect();
    assert_eq!(added_assets.len(), 0, "Alice should not receive any assets");
    assert_eq!(removed_assets.len(), 0, "Alice should not lose any assets");
    println!("Alice's vault unchanged — assets added inflight only");

    println!("\nMulti-note P2ID tx-script test passed!");
    println!("  - Note 1: {} USDC for Bob", amount1);
    println!("  - Note 2: {} ETH for Charlie", amount2);

    Ok(())
}
