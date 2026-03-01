use integration::helpers::{
    build_project_in_dir, create_testing_account_from_package, create_testing_note_from_package,
    AccountCreationConfig, NoteCreationConfig,
};

use miden_client::{
    note::{
        Note, NoteAssets, NoteExecutionHint, NoteInputs, NoteMetadata, NoteRecipient, NoteScript,
        NoteTag, NoteType,
    },
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
use std::{collections::BTreeMap, path::Path, sync::Arc};

use miden_swapp::PswapNote;

/// Compute the P2ID tag for a local account
fn compute_p2id_tag_for_local_account(account_id: AccountId) -> NoteTag {
    NoteTag::with_account_target(account_id)
}

/// Helper function to compute P2ID tag as Felt for use in note inputs
/// Returns the tag value as a Felt that can be directly added to note_inputs
fn compute_p2id_tag_felt(account_id: AccountId) -> Felt {
    let p2id_tag = compute_p2id_tag_for_local_account(account_id);
    // In v0.13, NoteTag is a newtype wrapper around u32
    // We can convert it using Into<u32>
    Felt::new(u32::from(p2id_tag) as u64)
}

fn create_swapp_note_with_pswap<R: miden_protocol::crypto::rand::FeltRng>(
    creator_account_id: AccountId,
    offered_asset: Asset,
    requested_asset: Asset,
    note_type: NoteType,
    rng: &mut R,
) -> anyhow::Result<Note> {
    let note = PswapNote::create(
        creator_account_id,
        offered_asset,
        requested_asset,
        note_type,
        NoteAttachment::default(),
        rng,
    )?;

    Ok(note)
}

#[tokio::test]
async fn swapp_note_full_fill_test() -> anyhow::Result<()> {
    println!("=== Test: Full Fill Swap ===");
    let mut builder = MockChain::builder();

    // STEP 1: Create faucets in genesis
    println!("Creating USDC and ETH faucets...");
    let usdc_faucet = builder.add_existing_basic_faucet(
        Auth::BasicAuth,
        "USDC",
        1000,      // max_supply
        Some(150), // total_issuance (50 for note + 100 for Bob)
    )?;
    println!("USDC Faucet: {:?}", usdc_faucet.id());
    println!("  Version: {:?}", usdc_faucet.id().version());

    let eth_faucet = builder.add_existing_basic_faucet(
        Auth::BasicAuth,
        "ETH",
        1000,     // max_supply
        Some(50), // total_issuance (25 for Alice's request)
    )?;
    println!("ETH Faucet: {:?}", eth_faucet.id());
    println!("  Version: {:?}", eth_faucet.id().version());

    // Print the prefix and suffix of the ETH faucet
    println!("ETH Faucet prefix: {:?}", eth_faucet.id().prefix());
    println!("ETH Faucet suffix: {:?}", eth_faucet.id().suffix());

    // Print the prefix and suffix of the USDC faucet
    println!("USDC Faucet prefix: {:?}", usdc_faucet.id().prefix());
    println!("USDC Faucet suffix: {:?}", usdc_faucet.id().suffix());

    // STEP 2: Create wallets with initial assets
    println!("\nCreating Alice and Bob wallets with initial assets...");
    let alice = builder.add_existing_wallet_with_assets(
        Auth::BasicAuth,
        [FungibleAsset::new(usdc_faucet.id(), 50)?.into()], // Alice has 50 USDC to offer
    )?;
    println!("Alice: {:?} (has 50 USDC)", alice.id());
    println!("  Version: {:?}", alice.id().version());

    // Build basic-wallet contract package
    println!("\nBuilding basic-wallet contract...");
    let account_package = Arc::new(build_project_in_dir(
        Path::new("../contracts/basic-wallet"),
        true,
    )?);
    println!("Basic-wallet contract built successfully.");

    // Create custom account configuration with default settings
    let bob_account_cfg = AccountCreationConfig {
        storage_slots: vec![],
        ..Default::default()
    };

    let assets = vec![FungibleAsset::new(eth_faucet.id(), 25)?.into()];

    let bob = create_testing_account_from_package(account_package.clone(), bob_account_cfg, assets)
        .await?;
    println!("Bob account created: {:?}", bob.id());

    let _bob_account = builder.add_account(bob.clone());

    // STEP 3: Build swapp-note and p2id-note contracts
    println!("\nBuilding swapp-note contract...");
    let swapp_package = Arc::new(build_project_in_dir(
        Path::new("../contracts/swapp-note"),
        true,
    )?);
    println!("Swapp note contract built successfully.");

    // STEP 4: Create swap note with proper structure
    println!("\nCreating swap note (Alice offers 50 USDC for 25 ETH)...");

    // Compute proper P2ID tag for Alice (who will receive the output note)
    let p2id_tag_felt = compute_p2id_tag_felt(alice.id());

    let note_inputs = vec![
        // Requested Asset (positions 0-3): 25 ETH
        eth_faucet.id().prefix().into(),
        eth_faucet.id().suffix(),
        Felt::ZERO,
        Felt::new(25), // requested_asset_total
        // Note Creator (positions 4-6): Alice
        alice.id().prefix().into(),
        alice.id().suffix(),
        NoteType::Public.into(),
        // P2ID Tag (position 7): computed tag for Alice
        p2id_tag_felt,
    ];

    // Add the offered asset (50 USDC) to the note
    let offered_asset = FungibleAsset::new(usdc_faucet.id(), 50)?;
    let mut note_assets = NoteAssets::default();
    note_assets.add_asset(offered_asset.into())?;

    let swap_note = create_testing_note_from_package(
        swapp_package.clone(),
        alice.id(),
        NoteCreationConfig {
            assets: note_assets,
            inputs: note_inputs,
            ..Default::default()
        },
    )?;
    println!("Swap note created: {:?}", swap_note.id());

    // Add note to genesis
    builder.add_output_note(OutputNote::Full(swap_note.clone()));

    // STEP 5: Build MockChain and execute transaction
    println!("\nBuilding MockChain...");
    let mut mock_chain = builder.build()?;

    // Bob consumes the swap note with full fill (25 ETH)
    println!("\nBob consuming swap note (providing 25 ETH - full fill)...");
    let note_args = Word::from([
        Felt::ZERO,
        Felt::ZERO,
        Felt::ZERO,
        Felt::new(25), // input_amount = 25 (full fill)
    ]);

    let mut note_args_map = BTreeMap::new();
    note_args_map.insert(swap_note.id(), note_args);

    // Create the expected P2ID note that will be created by the swap script
    // This note will contain 25 ETH and be sent to Alice
    println!("\nCreating expected P2ID note for Alice (25 ETH)...");

    let serial_num = Word::from([
        swap_note.recipient().serial_num()[0] + Felt::new(1),
        swap_note.recipient().serial_num()[1] + Felt::new(1),
        swap_note.recipient().serial_num()[2] + Felt::new(1),
        swap_note.recipient().serial_num()[3] + Felt::new(1),
    ]);

    let recipient = build_p2id_recipient(alice.id(), serial_num)?;

    // Prepare the advice map for the P2ID note
    // The key is the hash of the note creation parameters, and the value contains the full parameters
    let tag = compute_p2id_tag_for_local_account(alice.id());
    println!("Tag: {:?}", tag.as_u32());
    let aux = Felt::new(25);
    println!("recipient: {:?}", recipient.digest().to_hex());

    println!("serial num: {:?}", serial_num);

    let _execution_hint = NoteExecutionHint::none(); // Not used in v0.13

    // Add the asset (4 Felts = 1 Word)
    let asset = FungibleAsset::new(eth_faucet.id(), 25)?;

    let note_assets = NoteAssets::new(vec![asset.into()])?;

    // In v0.13, create metadata and attach the aux value using NoteAttachment
    // The aux value (25 ETH amount) is wrapped in a Word attachment
    let aux_word = Word::from([aux, Felt::ZERO, Felt::ZERO, Felt::ZERO]);
    let attachment = NoteAttachment::new_word(NoteAttachmentScheme::none(), aux_word);
    println!("Attachment word: {:?}", attachment.content());

    let note_metadata =
        NoteMetadata::new(bob.id(), NoteType::Public, tag).with_attachment(attachment.clone());

    let p2id_note = Note::new(note_assets, note_metadata, recipient.clone());

    // Rebuild the transaction context with the vault tree nodes
    let tx_context = mock_chain
        .build_tx_context(bob.id(), &[swap_note.id()], &[])?
        .extend_note_args(note_args_map)
        .extend_expected_output_notes(vec![OutputNote::Full(p2id_note.clone())])
        .build()?;

    let executed_transaction = tx_context.execute().await?;

    println!(
        "Cycle count: {:?}",
        executed_transaction.measurements().note_execution
    );

    // STEP 6: Verify results
    println!("\n=== Verification ===");

    // Check output notes - should have 1 P2ID note
    let output_notes = executed_transaction.output_notes();
    println!("Output notes created: {}", output_notes.num_notes());
    assert_eq!(output_notes.num_notes(), 1, "Expected exactly 1 P2ID note");

    assert_eq!(output_notes.get_note(0).recipient().unwrap(), &recipient);
    assert_eq!(output_notes.get_note(0).id(), p2id_note.id());

    assert_eq!(
        output_notes.get_note(0).metadata().attachment().clone(),
        attachment
    );

    let p2id_note = output_notes.get_note(0);
    println!("P2ID note created: {:?}", p2id_note.id());

    // Verify P2ID note contains 25 ETH for Alice
    let p2id_assets = p2id_note.assets().unwrap();
    assert_eq!(
        p2id_assets.num_assets(),
        1,
        "P2ID note should have exactly 1 asset"
    );

    // Get the asset and verify it's 25 ETH
    let p2id_asset = p2id_assets.iter().next().unwrap();
    let p2id_fungible = match p2id_asset {
        Asset::Fungible(f) => f,
        _ => panic!("Expected fungible asset in P2ID note"),
    };
    assert_eq!(
        p2id_fungible.faucet_id(),
        eth_faucet.id(),
        "P2ID note should contain ETH"
    );
    assert_eq!(
        p2id_fungible.amount(),
        25,
        "P2ID note should contain 25 ETH"
    );
    println!("✓ P2ID note verified: 25 ETH for Alice");

    // Check Bob's account delta - should have received 50 USDC
    let account_delta = executed_transaction.account_delta();
    let vault_delta = account_delta.vault();

    // Verify Bob received 50 USDC and spent 25 ETH
    let added_assets: Vec<Asset> = vault_delta.added_assets().collect();
    let removed_assets: Vec<Asset> = vault_delta.removed_assets().collect();

    assert_eq!(added_assets.len(), 1, "Bob should receive 1 asset (USDC)");
    assert_eq!(removed_assets.len(), 1, "Bob should spend 1 asset (ETH)");

    let usdc_received = match added_assets[0] {
        Asset::Fungible(f) => f,
        _ => panic!("Expected fungible USDC asset"),
    };
    assert_eq!(
        usdc_received.faucet_id(),
        usdc_faucet.id(),
        "Bob should receive USDC"
    );
    assert_eq!(usdc_received.amount(), 50, "Bob should receive 50 USDC");

    mock_chain.add_pending_executed_transaction(&executed_transaction)?;
    let _ = mock_chain.prove_next_block();

    let eth_spent = match removed_assets[0] {
        Asset::Fungible(f) => f,
        _ => panic!("Expected fungible ETH asset"),
    };
    assert_eq!(
        eth_spent.faucet_id(),
        eth_faucet.id(),
        "Bob should spend ETH"
    );
    assert_eq!(eth_spent.amount(), 25, "Bob should spend 25 ETH");
    println!("✓ Bob's vault delta verified: +50 USDC, -25 ETH");

    println!("\n✅ Full-fill swap test passed!");
    println!("  - Bob provided 25 ETH");
    println!("  - Bob received 50 USDC");
    println!("  - P2ID note created for Alice with 25 ETH");

    Ok(())
}

#[tokio::test]
async fn swapp_note_private_full_fill_test() -> anyhow::Result<()> {
    println!("=== Test: Private Note Full Fill Swap ===");
    let mut builder = MockChain::builder();

    // STEP 1: Create faucets in genesis
    println!("Creating USDC and ETH faucets...");
    let usdc_faucet = builder.add_existing_basic_faucet(
        Auth::BasicAuth,
        "USDC",
        1000,      // max_supply
        Some(150), // total_issuance (50 for note + 100 for Bob)
    )?;
    println!("USDC Faucet: {:?}", usdc_faucet.id());
    println!("  Version: {:?}", usdc_faucet.id().version());

    let eth_faucet = builder.add_existing_basic_faucet(
        Auth::BasicAuth,
        "ETH",
        1000,     // max_supply
        Some(50), // total_issuance (25 for Alice's request)
    )?;
    println!("ETH Faucet: {:?}", eth_faucet.id());
    println!("  Version: {:?}", eth_faucet.id().version());

    // STEP 2: Create wallets with initial assets
    println!("\nCreating Alice and Bob wallets with initial assets...");
    let alice = builder.add_existing_wallet_with_assets(
        Auth::BasicAuth,
        [FungibleAsset::new(usdc_faucet.id(), 50)?.into()], // Alice has 50 USDC to offer
    )?;
    println!("Alice: {:?} (has 50 USDC)", alice.id());
    println!("  Version: {:?}", alice.id().version());

    // Build basic-wallet contract package
    println!("\nBuilding basic-wallet contract...");
    let account_package = Arc::new(build_project_in_dir(
        Path::new("../contracts/basic-wallet"),
        true,
    )?);
    println!("Basic-wallet contract built successfully.");

    // Create custom account configuration with default settings
    let bob_account_cfg = AccountCreationConfig {
        storage_slots: vec![],
        ..Default::default()
    };

    let assets = vec![FungibleAsset::new(eth_faucet.id(), 25)?.into()];

    let bob = create_testing_account_from_package(account_package.clone(), bob_account_cfg, assets)
        .await?;
    println!("Bob account created: {:?}", bob.id());

    let _bob_account = builder.add_account(bob.clone());

    // STEP 3: Build swapp-note contract
    println!("\nBuilding swapp-note contract...");
    let swapp_package = Arc::new(build_project_in_dir(
        Path::new("../contracts/swapp-note"),
        true,
    )?);
    println!("Swapp note contract built successfully.");

    // STEP 4: Create PRIVATE swap note with proper structure
    println!("\nCreating PRIVATE swap note (Alice offers 50 USDC for 25 ETH)...");

    // Compute proper P2ID tag for Alice (who will receive the output note)
    let p2id_tag_felt = compute_p2id_tag_felt(alice.id());

    let note_inputs = vec![
        // Requested Asset (positions 0-3): 25 ETH
        eth_faucet.id().prefix().into(),
        eth_faucet.id().suffix(),
        Felt::ZERO,
        Felt::new(25), // requested_asset_total
        // Note Creator (positions 4-6): Alice
        alice.id().prefix().into(),
        alice.id().suffix(),
        NoteType::Private.into(),
        // P2ID Tag (position 7): computed tag for Alice
        p2id_tag_felt,
    ];

    // Add the offered asset (50 USDC) to the note
    let offered_asset = FungibleAsset::new(usdc_faucet.id(), 50)?;
    let mut note_assets = NoteAssets::default();
    note_assets.add_asset(offered_asset.into())?;

    let swap_note = create_testing_note_from_package(
        swapp_package.clone(),
        alice.id(),
        NoteCreationConfig {
            assets: note_assets,
            inputs: note_inputs,
            note_type: NoteType::Private, // PRIVATE NOTE
            ..Default::default()
        },
    )?;
    println!("Private swap note created: {:?}", swap_note.id());
    println!("  Note type: {:?}", swap_note.metadata().note_type());

    // Add note to genesis
    builder.add_output_note(OutputNote::Full(swap_note.clone()));

    // STEP 5: Build MockChain and execute transaction
    println!("\nBuilding MockChain...");
    let mut mock_chain = builder.build()?;

    // Bob consumes the swap note with full fill (25 ETH)
    println!("\nBob consuming private swap note (providing 25 ETH - full fill)...");
    let note_args = Word::from([
        Felt::ZERO,
        Felt::ZERO,
        Felt::ZERO,
        Felt::new(25), // input_amount = 25 (full fill)
    ]);

    let mut note_args_map = BTreeMap::new();
    note_args_map.insert(swap_note.id(), note_args);

    // Create the expected P2ID note that will be created by the swap script
    // This note will contain 25 ETH and be sent to Alice
    println!("\nCreating expected P2ID note for Alice (25 ETH)...");

    let serial_num = Word::from([
        swap_note.recipient().serial_num()[0] + Felt::new(1),
        swap_note.recipient().serial_num()[1] + Felt::new(1),
        swap_note.recipient().serial_num()[2] + Felt::new(1),
        swap_note.recipient().serial_num()[3] + Felt::new(1),
    ]);

    let recipient = build_p2id_recipient(alice.id(), serial_num)?;

    // Prepare the advice map for the P2ID note
    let tag = compute_p2id_tag_for_local_account(alice.id());
    let aux = Felt::new(25);
    let _execution_hint = NoteExecutionHint::none(); // Not used in v0.13

    // Add the asset (4 Felts = 1 Word)
    let asset = FungibleAsset::new(eth_faucet.id(), 25)?;

    let note_assets = NoteAssets::new(vec![asset.into()])?;

    // P2ID note should also be private to match privacy expectations
    // In v0.13, attach the aux value using NoteAttachment
    let aux_word = Word::from([aux, Felt::ZERO, Felt::ZERO, Felt::ZERO]);
    let attachment = NoteAttachment::new_word(NoteAttachmentScheme::none(), aux_word);
    let note_metadata =
        NoteMetadata::new(bob.id(), NoteType::Private, tag).with_attachment(attachment);

    let p2id_note = Note::new(note_assets, note_metadata, recipient);

    // Rebuild the transaction context with the vault tree nodes
    let tx_context = mock_chain
        .build_tx_context(bob.id(), &[swap_note.id()], &[])?
        .extend_note_args(note_args_map)
        .extend_expected_output_notes(vec![OutputNote::Full(p2id_note)])
        .build()?;

    let executed_transaction = tx_context.execute().await?;

    println!(
        "Cycle count: {:?}",
        executed_transaction.measurements().note_execution
    );

    // STEP 6: Verify results
    println!("\n=== Verification ===");

    // Check output notes - should have 1 P2ID note
    let output_notes = executed_transaction.output_notes();
    println!("Output notes created: {}", output_notes.num_notes());
    assert_eq!(output_notes.num_notes(), 1, "Expected exactly 1 P2ID note");

    let p2id_note = output_notes.get_note(0);
    println!("P2ID note created: {:?}", p2id_note.id());

    // Verify the note is private
    assert_eq!(
        p2id_note.metadata().note_type(),
        NoteType::Private,
        "P2ID note should be private"
    );
    println!("✓ P2ID note is private");

    // Verify P2ID note contains 25 ETH for Alice
    let p2id_assets = p2id_note.assets().unwrap();
    assert_eq!(
        p2id_assets.num_assets(),
        1,
        "P2ID note should have exactly 1 asset"
    );

    // Get the asset and verify it's 25 ETH
    let p2id_asset = p2id_assets.iter().next().unwrap();
    let p2id_fungible = match p2id_asset {
        Asset::Fungible(f) => f,
        _ => panic!("Expected fungible asset in P2ID note"),
    };
    assert_eq!(
        p2id_fungible.faucet_id(),
        eth_faucet.id(),
        "P2ID note should contain ETH"
    );
    assert_eq!(
        p2id_fungible.amount(),
        25,
        "P2ID note should contain 25 ETH"
    );
    println!("✓ P2ID note verified: 25 ETH for Alice");

    // Check Bob's account delta - should have received 50 USDC
    let account_delta = executed_transaction.account_delta();
    let vault_delta = account_delta.vault();

    // Verify Bob received 50 USDC and spent 25 ETH
    let added_assets: Vec<Asset> = vault_delta.added_assets().collect();
    let removed_assets: Vec<Asset> = vault_delta.removed_assets().collect();

    assert_eq!(added_assets.len(), 1, "Bob should receive 1 asset (USDC)");

    let usdc_received = match added_assets[0] {
        Asset::Fungible(f) => f,
        _ => panic!("Expected fungible USDC asset"),
    };
    assert_eq!(
        usdc_received.faucet_id(),
        usdc_faucet.id(),
        "Bob should receive USDC"
    );
    assert_eq!(usdc_received.amount(), 50, "Bob should receive 50 USDC");

    mock_chain.add_pending_executed_transaction(&executed_transaction)?;
    let _ = mock_chain.prove_next_block();

    let eth_spent = match removed_assets[0] {
        Asset::Fungible(f) => f,
        _ => panic!("Expected fungible ETH asset"),
    };
    assert_eq!(
        eth_spent.faucet_id(),
        eth_faucet.id(),
        "Bob should spend ETH"
    );
    assert_eq!(eth_spent.amount(), 25, "Bob should spend 25 ETH");
    println!("✓ Bob's vault delta verified: +50 USDC, -25 ETH");

    println!("\n✅ Private note full-fill swap test passed!");
    println!("  - Private swap note created and consumed");
    println!("  - Bob provided 25 ETH");
    println!("  - Bob received 50 USDC");
    println!("  - Private P2ID note created for Alice with 25 ETH");

    Ok(())
}

#[tokio::test]
async fn swapp_note_partial_fill_test() -> anyhow::Result<()> {
    println!("=== Test: Partial Fill Swap ===");
    let mut builder = MockChain::builder();

    // STEP 1: Create faucets in genesis
    println!("Creating USDC and ETH faucets...");
    let usdc_faucet = builder.add_existing_basic_faucet(
        Auth::BasicAuth,
        "USDC",
        1000,      // max_supply
        Some(150), // total_issuance (10 for note + 100 for Bob)
    )?;
    println!("USDC Faucet: {:?}", usdc_faucet.id());
    println!("  Version: {:?}", usdc_faucet.id().version());

    let eth_faucet = builder.add_existing_basic_faucet(
        Auth::BasicAuth,
        "ETH",
        1000,     // max_supply
        Some(50), // total_issuance (3 for Alice's request)
    )?;
    println!("ETH Faucet: {:?}", eth_faucet.id());
    println!("  Version: {:?}", eth_faucet.id().version());

    // STEP 2: Create wallets with initial assets
    println!("\nCreating Alice and Bob wallets with initial assets...");
    let alice = builder.add_existing_wallet_with_assets(
        Auth::BasicAuth,
        [FungibleAsset::new(usdc_faucet.id(), 10)?.into()], // Alice has 10 USDC to offer
    )?;
    println!("Alice: {:?} (has 10 USDC)", alice.id());
    println!("  Version: {:?}", alice.id().version());

    // Build basic-wallet contract package
    println!("\nBuilding basic-wallet contract...");
    let account_package = Arc::new(build_project_in_dir(
        Path::new("../contracts/basic-wallet"),
        true,
    )?);
    println!("Basic-wallet contract built successfully.");

    // Create custom account configuration with default settings
    let bob_account_cfg = AccountCreationConfig {
        storage_slots: vec![],
        ..Default::default()
    };

    let assets = vec![FungibleAsset::new(eth_faucet.id(), 3)?.into()]; // Bob has 3 ETH to provide

    let bob = create_testing_account_from_package(account_package.clone(), bob_account_cfg, assets)
        .await?;
    println!("Bob account created: {:?} (has 3 ETH)", bob.id());

    let _bob_account = builder.add_account(bob.clone());

    // STEP 3: Build swapp-note and p2id-note contracts
    println!("\nBuilding swapp-note contract...");
    let swapp_package = Arc::new(build_project_in_dir(
        Path::new("../contracts/swapp-note"),
        true,
    )?);
    println!("Swapp note contract built successfully.");

    // STEP 4: Create swap note with proper structure
    println!("\nCreating swap note (Alice offers 10 USDC for 3 ETH)...");

    // Compute proper P2ID tag for Alice (who will receive the output note)
    let p2id_tag_felt = compute_p2id_tag_felt(alice.id());

    let note_inputs = vec![
        // Requested Asset (positions 0-3): 3 ETH
        eth_faucet.id().prefix().into(),
        eth_faucet.id().suffix(),
        Felt::ZERO,
        Felt::new(3), // requested_asset_total
        // Note Creator (positions 4-6): Alice
        alice.id().prefix().into(),
        alice.id().suffix(),
        NoteType::Public.into(),
        // P2ID Tag (position 7): computed tag for Alice
        p2id_tag_felt,
    ];

    // Add the offered asset (10 USDC) to the note
    let offered_asset = FungibleAsset::new(usdc_faucet.id(), 10)?;
    let mut note_assets = NoteAssets::default();
    note_assets.add_asset(offered_asset.into())?;

    let swap_note = create_testing_note_from_package(
        swapp_package.clone(),
        alice.id(),
        NoteCreationConfig {
            assets: note_assets,
            inputs: note_inputs,
            ..Default::default()
        },
    )?;
    println!("Swap note created: {:?}", swap_note.id());

    // Add note to genesis
    builder.add_output_note(OutputNote::Full(swap_note.clone()));

    // STEP 5: Build MockChain and execute transaction
    println!("\nBuilding MockChain...");
    let mock_chain = builder.build()?;

    println!("\nBob consuming swap note (providing 1 ETH - partial fill)...");
    let note_args = Word::from([
        Felt::ZERO, // input_amount = 1 (partial fill, 33% of requested)
        Felt::ZERO,
        Felt::ZERO,
        Felt::new(1),
    ]);

    let mut note_args_map = BTreeMap::new();
    note_args_map.insert(swap_note.id(), note_args);

    // Create the expected P2ID note that will be created by the swap script
    // This note will contain 1 ETH (input_amount) and be sent to Alice
    println!("\nCreating expected P2ID note for Alice (1 ETH)...");

    let p2id_serial_num = Word::from([
        swap_note.recipient().serial_num()[0] + Felt::new(1),
        swap_note.recipient().serial_num()[1] + Felt::new(1),
        swap_note.recipient().serial_num()[2] + Felt::new(1),
        swap_note.recipient().serial_num()[3] + Felt::new(1),
    ]);

    let p2id_recipient = build_p2id_recipient(alice.id(), p2id_serial_num)?;

    let p2id_tag = compute_p2id_tag_for_local_account(alice.id());
    let p2id_aux = Felt::new(1); // input_amount = 1 ETH

    let p2id_asset = FungibleAsset::new(eth_faucet.id(), 1)?; // 1 ETH
    let p2id_note_assets = NoteAssets::new(vec![p2id_asset.into()])?;

    // Attach aux value (1) to the metadata
    let aux_word = Word::from([p2id_aux, Felt::ZERO, Felt::ZERO, Felt::ZERO]);
    let attachment = NoteAttachment::new_word(NoteAttachmentScheme::none(), aux_word);
    let p2id_note_metadata =
        NoteMetadata::new(bob.id(), NoteType::Public, p2id_tag).with_attachment(attachment);

    let p2id_note = Note::new(p2id_note_assets, p2id_note_metadata, p2id_recipient);

    // Create the expected remainder swap note (contains 7 USDC, requests 2 ETH)
    println!("\nCreating expected remainder swap note (7 USDC for 2 ETH)...");

    let current_note_serial = swap_note.recipient().serial_num();
    let serial_num_word = current_note_serial;
    let serial_num_array: [Felt; 4] = serial_num_word.into();
    // Serial number is RPO hash of the existing swap note's serial number
    let remainder_serial_num: [Felt; 4] =
        miden_core::crypto::hash::Rpo256::hash_elements(&serial_num_array).into();
    let remainder_serial_num = Word::from(remainder_serial_num);
    println!("Remainder serial num: {:?}", remainder_serial_num);

    // Note inputs for remainder swap note: requesting 2 ETH, creator is Alice
    let remainder_note_inputs = vec![
        // Requested Asset (positions 0-3): 2 ETH (remaining)
        eth_faucet.id().prefix().into(),
        eth_faucet.id().suffix(),
        Felt::ZERO,
        Felt::new(2), // requested_asset_total (3 - 1 = 2)
        // Note Creator (positions 4-7): Alice
        alice.id().prefix().into(),
        alice.id().suffix(),
        NoteType::Public.into(),
        p2id_tag_felt,
    ];

    // Create recipient for remainder swap note
    let note_program = swapp_package.unwrap_program();
    let remainder_note_script = NoteScript::from_parts(
        note_program.mast_forest().clone(),
        note_program.entrypoint(),
    );
    let remainder_note_inputs_obj = NoteInputs::new(remainder_note_inputs)?;
    let remainder_recipient = NoteRecipient::new(
        remainder_serial_num,
        remainder_note_script,
        remainder_note_inputs_obj,
    );

    // Create metadata for remainder note with aux attachment
    let remainder_tag = swap_note.metadata().tag();
    let remainder_aux = Felt::new(3); // offered_out = (10 * 1) / 3 = 3

    println!(
        "Remainder recipient: {:?}",
        remainder_recipient.digest().to_hex()
    );

    // Attach aux value (3) to the remainder note
    let aux_word = Word::from([remainder_aux, Felt::ZERO, Felt::ZERO, Felt::ZERO]);
    let attachment = NoteAttachment::new_word(NoteAttachmentScheme::none(), aux_word);
    let remainder_note_metadata =
        NoteMetadata::new(bob.id(), NoteType::Public, remainder_tag).with_attachment(attachment);

    // Create assets for remainder note: 7 USDC (10 - 3 = 7)
    let remainder_asset = FungibleAsset::new(usdc_faucet.id(), 7)?;
    let remainder_note_assets = NoteAssets::new(vec![remainder_asset.into()])?;

    let remainder_note = Note::new(
        remainder_note_assets,
        remainder_note_metadata,
        remainder_recipient,
    );

    let tx_context = mock_chain
        .build_tx_context(bob.id(), &[swap_note.id()], &[])?
        .extend_expected_output_notes(vec![
            OutputNote::Full(p2id_note),
            OutputNote::Full(remainder_note),
        ])
        .extend_note_args(note_args_map)
        .build()?;

    let executed_transaction = tx_context.execute().await?;
    println!("Transaction executed successfully!");

    println!(
        "Cycle count: {:?}",
        executed_transaction.measurements().note_execution
    );

    // STEP 6: Verify results
    println!("\n=== Verification ===");

    // Should have 2 output notes: P2ID note + remainder swap note
    let output_notes = executed_transaction.output_notes();
    println!("Output notes created: {}", output_notes.num_notes());
    assert_eq!(
        output_notes.num_notes(),
        2,
        "Expected 2 notes: 1 P2ID + 1 remainder"
    );

    // Find P2ID note and remainder note
    let mut p2id_note_found = false;
    let mut remainder_note_found = false;

    for idx in 0..output_notes.num_notes() {
        let note = output_notes.get_note(idx);
        let assets = note.assets().unwrap();
        if assets.num_assets() == 1 {
            let asset = assets.iter().next().unwrap();
            let fungible = match asset {
                Asset::Fungible(f) => f,
                _ => continue,
            };

            if fungible.faucet_id() == eth_faucet.id() {
                // This is the P2ID note (contains ETH)
                assert_eq!(fungible.amount(), 1, "P2ID note should contain 1 ETH");
                println!("✓ P2ID note verified: 1 ETH for Alice");
                p2id_note_found = true;
            } else if fungible.faucet_id() == usdc_faucet.id() {
                // This is the remainder swap note (contains remaining USDC)
                // Expected: offered_out = (10 * 1) / 3 = 3, so remaining = 10 - 3 = 7
                assert_eq!(fungible.amount(), 7, "Remainder note should contain 7 USDC");
                println!("✓ Remainder note verified: 7 USDC (still requesting 2 ETH)");
                remainder_note_found = true;
            }
        }
    }

    assert!(p2id_note_found, "P2ID note not found");
    assert!(remainder_note_found, "Remainder swap note not found");

    // Check Bob's vault delta
    let account_delta = executed_transaction.account_delta();
    let vault_delta = account_delta.vault();
    let added_assets: Vec<Asset> = vault_delta.added_assets().collect();

    assert_eq!(added_assets.len(), 1, "Bob should receive 1 asset");
    let usdc_received = match added_assets[0] {
        Asset::Fungible(f) => f,
        _ => panic!("Expected fungible USDC asset"),
    };
    assert_eq!(usdc_received.amount(), 3, "Bob should receive 3 USDC");
    println!("✓ Bob's vault delta verified: +3 USDC, -1 ETH");

    println!("\n✅ Partial-fill swap test passed!");
    println!("  - Bob provided 1 ETH (33% of requested 3)");
    println!("  - Bob received 3 USDC (33% of offered 10)");
    println!("  - P2ID note created for Alice with 1 ETH");
    println!("  - Remainder swap note created: 7 USDC for 2 ETH");

    Ok(())
}

#[tokio::test]
async fn swapp_note_multiple_partial_fills_test() -> anyhow::Result<()> {
    println!("=== Test: Multiple Partial Fill Scenarios ===");

    // Helper function to calculate output amount (matching the contract logic)
    fn calculate_output_amount(offered_total: u64, requested_total: u64, input_amount: u64) -> u64 {
        let precision_factor = 100000u64;

        if offered_total > requested_total {
            // Case 1: offered_total > requested_total
            let ratio = (offered_total * precision_factor) / requested_total;
            (input_amount * ratio) / precision_factor
        } else {
            // Case 2: offered_total <= requested_total
            let ratio = (requested_total * precision_factor) / offered_total;
            (input_amount * precision_factor) / ratio
        }
    }

    // Test scenarios with different input amounts
    let test_scenarios = vec![
        (5, "5 ETH - 20% fill"),
        (7, "7 ETH - 28% fill"),
        (10, "10 ETH - 40% fill"),
        (13, "13 ETH - 52% fill"),
        (15, "15 ETH - 60% fill"),
        (19, "19 ETH - 76% fill"),
        (20, "20 ETH - 80% fill"),
        (23, "23 ETH - 92% fill"),
        (25, "25 ETH - 100% fill (full)"),
    ];

    for (input_amount, description) in test_scenarios {
        println!("\n============================================================");
        println!("Testing scenario: {}", description);
        println!("============================================================");

        let mut builder = MockChain::builder();

        // STEP 1: Create faucets in genesis
        println!("Creating USDC and ETH faucets...");
        let usdc_faucet = builder.add_existing_basic_faucet(
            Auth::BasicAuth,
            "USDC",
            1000,
            Some(150), // total_issuance
        )?;

        let eth_faucet = builder.add_existing_basic_faucet(
            Auth::BasicAuth,
            "ETH",
            1000,
            Some(50), // total_issuance
        )?;

        // STEP 2: Create Alice wallet
        let alice = builder.add_existing_wallet_with_assets(
            Auth::BasicAuth,
            [FungibleAsset::new(usdc_faucet.id(), 50)?.into()],
        )?;
        println!("Alice: {:?} (has 50 USDC)", alice.id());

        // Build basic-wallet contract package
        let account_package = Arc::new(build_project_in_dir(
            Path::new("../contracts/basic-wallet"),
            true,
        )?);

        // Create Bob's custom account with ETH
        let bob_account_cfg = AccountCreationConfig {
            storage_slots: vec![],
            ..Default::default()
        };

        let assets = vec![FungibleAsset::new(eth_faucet.id(), input_amount)?.into()];
        let bob =
            create_testing_account_from_package(account_package.clone(), bob_account_cfg, assets)
                .await?;
        println!(
            "Bob account created with {} ETH: {:?}",
            input_amount,
            bob.id()
        );

        builder.add_account(bob.clone())?;

        // STEP 3: Build swapp-note contract
        let swapp_package = Arc::new(build_project_in_dir(
            Path::new("../contracts/swapp-note"),
            true,
        )?);

        // STEP 4: Create swap note (Alice offers 50 USDC for 25 ETH)
        println!("\nCreating swap note (Alice offers 50 USDC for 25 ETH)...");

        // Compute proper P2ID tag for Alice (who will receive the output note)
        let p2id_tag_felt = compute_p2id_tag_felt(alice.id());

        let note_inputs = vec![
            eth_faucet.id().prefix().into(),
            eth_faucet.id().suffix(),
            Felt::ZERO,
            Felt::new(25), // requested_asset_total
            alice.id().prefix().into(),
            alice.id().suffix(),
            NoteType::Public.into(),
            // P2ID Tag (position 7): computed tag for Alice
            p2id_tag_felt,
        ];

        let offered_asset = FungibleAsset::new(usdc_faucet.id(), 50)?;
        let mut note_assets = NoteAssets::default();
        note_assets.add_asset(offered_asset.into())?;

        let swap_note = create_testing_note_from_package(
            swapp_package.clone(),
            alice.id(),
            NoteCreationConfig {
                assets: note_assets,
                inputs: note_inputs,
                ..Default::default()
            },
        )?;

        builder.add_output_note(OutputNote::Full(swap_note.clone()));

        // STEP 5: Build MockChain
        let mock_chain = builder.build()?;

        // Calculate expected amounts
        let offered_total = 50u64;
        let requested_total = 25u64;
        let offered_out = calculate_output_amount(offered_total, requested_total, input_amount);
        let remaining_usdc = offered_total - offered_out;
        let remaining_eth = requested_total - input_amount;

        println!("\nExpected calculations:");
        println!("  Input amount: {} ETH", input_amount);
        println!("  Offered out (to Bob): {} USDC", offered_out);
        println!("  Remaining USDC: {}", remaining_usdc);
        println!("  Remaining ETH requested: {}", remaining_eth);

        // Execute transaction
        println!(
            "\nBob consuming swap note (providing {} ETH)...",
            input_amount
        );
        let note_args = Word::from([Felt::ZERO, Felt::ZERO, Felt::ZERO, Felt::new(input_amount)]);

        let mut note_args_map = BTreeMap::new();
        note_args_map.insert(swap_note.id(), note_args);

        // Create expected P2ID note
        let p2id_serial_num = Word::from([
            swap_note.recipient().serial_num()[0] + Felt::new(1),
            swap_note.recipient().serial_num()[1] + Felt::new(1),
            swap_note.recipient().serial_num()[2] + Felt::new(1),
            swap_note.recipient().serial_num()[3] + Felt::new(1),
        ]);

        let p2id_recipient = build_p2id_recipient(alice.id(), p2id_serial_num)?;
        let p2id_tag = compute_p2id_tag_for_local_account(alice.id());
        let p2id_aux = Felt::new(input_amount);
        let p2id_asset = FungibleAsset::new(eth_faucet.id(), input_amount)?;
        let p2id_note_assets = NoteAssets::new(vec![p2id_asset.into()])?;

        // Attach aux value to the metadata
        let aux_word = Word::from([p2id_aux, Felt::ZERO, Felt::ZERO, Felt::ZERO]);
        let attachment = NoteAttachment::new_word(NoteAttachmentScheme::none(), aux_word);
        let p2id_note_metadata =
            NoteMetadata::new(bob.id(), NoteType::Public, p2id_tag).with_attachment(attachment);
        let p2id_note = Note::new(p2id_note_assets, p2id_note_metadata, p2id_recipient);

        let mut expected_notes = vec![OutputNote::Full(p2id_note)];

        // Create expected remainder swap note (only if not full fill)
        if input_amount < 25 {
            println!(
                "\nCreating expected remainder swap note ({} USDC for {} ETH)...",
                remaining_usdc, remaining_eth
            );

            let current_note_serial = swap_note.recipient().serial_num();
            let serial_num_array: [Felt; 4] = current_note_serial.into();
            let remainder_serial_num: [Felt; 4] =
                miden_core::crypto::hash::Rpo256::hash_elements(&serial_num_array).into();
            let remainder_serial_num = Word::from(remainder_serial_num);

            let remainder_note_inputs = vec![
                eth_faucet.id().prefix().into(),
                eth_faucet.id().suffix(),
                Felt::ZERO,
                Felt::new(remaining_eth),
                alice.id().prefix().into(),
                alice.id().suffix(),
                NoteType::Public.into(),
                p2id_tag_felt,
            ];

            let note_program = swapp_package.unwrap_program();
            let remainder_note_script = NoteScript::from_parts(
                note_program.mast_forest().clone(),
                note_program.entrypoint(),
            );
            let remainder_note_inputs_obj = NoteInputs::new(remainder_note_inputs)?;
            let remainder_recipient = NoteRecipient::new(
                remainder_serial_num,
                remainder_note_script,
                remainder_note_inputs_obj,
            );

            let remainder_tag = swap_note.metadata().tag();
            let remainder_aux = Felt::new(offered_out);

            // Attach aux value to the remainder note metadata
            let aux_word = Word::from([remainder_aux, Felt::ZERO, Felt::ZERO, Felt::ZERO]);
            let attachment = NoteAttachment::new_word(NoteAttachmentScheme::none(), aux_word);
            let remainder_note_metadata =
                NoteMetadata::new(bob.id(), NoteType::Public, remainder_tag)
                    .with_attachment(attachment);

            let remainder_asset = FungibleAsset::new(usdc_faucet.id(), remaining_usdc)?;
            let remainder_note_assets = NoteAssets::new(vec![remainder_asset.into()])?;

            let remainder_note = Note::new(
                remainder_note_assets,
                remainder_note_metadata,
                remainder_recipient,
            );

            expected_notes.push(OutputNote::Full(remainder_note));
        }

        let tx_context = mock_chain
            .build_tx_context(bob.id(), &[swap_note.id()], &[])?
            .extend_expected_output_notes(expected_notes)
            .extend_note_args(note_args_map)
            .build()?;

        let executed_transaction = tx_context.execute().await?;
        println!("Transaction executed successfully!");

        println!(
            "Cycle count: {:?}",
            executed_transaction.measurements().note_execution
        );

        // STEP 6: Verify results
        println!("\n=== Verification ===");

        let output_notes = executed_transaction.output_notes();
        let expected_note_count = if input_amount < 25 { 2 } else { 1 };
        println!("Output notes created: {}", output_notes.num_notes());
        assert_eq!(
            output_notes.num_notes(),
            expected_note_count,
            "Expected {} output note(s)",
            expected_note_count
        );

        // Verify P2ID note and remainder note
        let mut p2id_found = false;
        let mut remainder_found = false;

        for idx in 0..output_notes.num_notes() {
            let note = output_notes.get_note(idx);
            let assets = note.assets().unwrap();

            if assets.num_assets() == 1 {
                let asset = assets.iter().next().unwrap();
                if let Asset::Fungible(f) = asset {
                    if f.faucet_id() == eth_faucet.id() {
                        assert_eq!(
                            f.amount(),
                            input_amount,
                            "P2ID note should contain {} ETH",
                            input_amount
                        );
                        println!("  ✓ P2ID note verified: {} ETH for Alice", input_amount);
                        p2id_found = true;
                    } else if f.faucet_id() == usdc_faucet.id() {
                        assert_eq!(
                            f.amount(),
                            remaining_usdc,
                            "Remainder note should contain {} USDC",
                            remaining_usdc
                        );
                        println!(
                            "  ✓ Remainder note verified: {} USDC (requesting {} ETH)",
                            remaining_usdc, remaining_eth
                        );
                        remainder_found = true;
                    }
                }
            }
        }

        assert!(p2id_found, "P2ID note not found");
        if input_amount < 25 {
            assert!(remainder_found, "Remainder swap note not found");
        }

        // Check Bob's vault delta
        let account_delta = executed_transaction.account_delta();
        let vault_delta = account_delta.vault();
        let added_assets: Vec<Asset> = vault_delta.added_assets().collect();

        assert_eq!(added_assets.len(), 1, "Bob should receive 1 asset");
        if let Asset::Fungible(f) = added_assets[0] {
            assert_eq!(
                f.amount(),
                offered_out,
                "Bob should receive {} USDC",
                offered_out
            );
            println!("  ✓ Bob received {} USDC", offered_out);
        }

        println!("\n✅ Scenario '{}' passed!", description);
    }

    println!("\n============================================================");
    println!("✅ All multiple partial fill scenarios passed!");
    println!("============================================================");

    Ok(())
}

#[tokio::test]
async fn swapp_note_inflight_cross_swap_test() -> anyhow::Result<()> {
    println!("=== Test: Inflight Cross Swap (Two Notes, Zero Input) ===");
    let mut builder = MockChain::builder();

    // STEP 1: Create faucets in genesis
    println!("Creating USDC and ETH faucets...");
    let usdc_faucet = builder.add_existing_basic_faucet(
        Auth::BasicAuth,
        "USDC",
        1000,
        Some(100), // total_issuance
    )?;
    println!("USDC Faucet: {:?}", usdc_faucet.id());

    let eth_faucet = builder.add_existing_basic_faucet(
        Auth::BasicAuth,
        "ETH",
        1000,
        Some(50), // total_issuance
    )?;
    println!("ETH Faucet: {:?}", eth_faucet.id());

    // STEP 2: Create Alice, Charlie, and Bob accounts
    println!("\nCreating Alice with 25 ETH (wants 50 USDC)...");
    let alice = builder.add_existing_wallet_with_assets(
        Auth::BasicAuth,
        [FungibleAsset::new(eth_faucet.id(), 25)?.into()],
    )?;
    println!("Alice: {:?} (has 25 ETH)", alice.id());

    println!("\nCreating Charlie with 50 USDC (wants 25 ETH)...");
    let charlie = builder.add_existing_wallet_with_assets(
        Auth::BasicAuth,
        [FungibleAsset::new(usdc_faucet.id(), 50)?.into()],
    )?;
    println!("Charlie: {:?} (has 50 USDC)", charlie.id());

    // Build basic-wallet contract package
    println!("\nBuilding basic-wallet contract...");
    let account_package = Arc::new(build_project_in_dir(
        Path::new("../contracts/basic-wallet"),
        true,
    )?);
    println!("Basic-wallet contract built successfully.");

    // Create Bob's custom account with 0 assets
    println!("\nCreating Bob with 0 assets (custom account)...");
    let bob_account_cfg = AccountCreationConfig {
        storage_slots: vec![],
        ..Default::default()
    };

    let bob = create_testing_account_from_package(
        account_package.clone(),
        bob_account_cfg,
        vec![], // 0 assets
    )
    .await?;
    println!("Bob account created: {:?}", bob.id());

    builder.add_account(bob.clone())?;

    // STEP 3: Build swapp-note contract
    println!("\nBuilding swapp-note contract...");
    let swapp_package = Arc::new(build_project_in_dir(
        Path::new("../contracts/swapp-note"),
        true,
    )?);
    println!("Swapp note contract built successfully.");

    // STEP 4: Create Alice's swap note (offers 25 ETH, wants 50 USDC)
    println!("\nCreating Alice's swap note (offers 25 ETH for 50 USDC)...");

    // Compute proper P2ID tag for Alice (who will receive the output note)
    let alice_p2id_tag_felt = compute_p2id_tag_felt(alice.id());

    let alice_note_inputs = vec![
        // Requested Asset: 50 USDC
        usdc_faucet.id().prefix().into(),
        usdc_faucet.id().suffix(),
        Felt::ZERO,
        Felt::new(50), // requested_asset_total
        // Note Creator: Alice
        alice.id().prefix().into(),
        alice.id().suffix(),
        NoteType::Public.into(),
        // P2ID Tag (position 7): computed tag for Alice
        alice_p2id_tag_felt,
    ];

    let alice_offered_asset = FungibleAsset::new(eth_faucet.id(), 25)?;
    let mut alice_note_assets = NoteAssets::default();
    alice_note_assets.add_asset(alice_offered_asset.into())?;

    let alice_swap_note = create_testing_note_from_package(
        swapp_package.clone(),
        alice.id(),
        NoteCreationConfig {
            assets: alice_note_assets,
            inputs: alice_note_inputs,
            ..Default::default()
        },
    )?;
    println!("Alice's swap note created: {:?}", alice_swap_note.id());

    builder.add_output_note(OutputNote::Full(alice_swap_note.clone()));

    // STEP 5: Create Charlie's swap note (offers 50 USDC, wants 25 ETH)
    println!("\nCreating Charlie's swap note (offers 50 USDC for 25 ETH)...");

    // Compute proper P2ID tag for Charlie (who will receive the output note)
    let charlie_p2id_tag_felt = compute_p2id_tag_felt(charlie.id());

    let charlie_note_inputs = vec![
        // Requested Asset: 25 ETH
        eth_faucet.id().prefix().into(),
        eth_faucet.id().suffix(),
        Felt::ZERO,
        Felt::new(25), // requested_asset_total
        // Note Creator: Charlie
        charlie.id().prefix().into(),
        charlie.id().suffix(),
        NoteType::Public.into(),
        // P2ID Tag (position 7): computed tag for Charlie
        charlie_p2id_tag_felt,
    ];

    let charlie_offered_asset = FungibleAsset::new(usdc_faucet.id(), 50)?;
    let mut charlie_note_assets = NoteAssets::default();
    charlie_note_assets.add_asset(charlie_offered_asset.into())?;

    let charlie_swap_note = create_testing_note_from_package(
        swapp_package.clone(),
        charlie.id(),
        NoteCreationConfig {
            assets: charlie_note_assets,
            inputs: charlie_note_inputs,
            ..Default::default()
        },
    )?;
    println!("Charlie's swap note created: {:?}", charlie_swap_note.id());

    builder.add_output_note(OutputNote::Full(charlie_swap_note.clone()));

    // STEP 6: Build MockChain
    println!("\nBuilding MockChain...");
    let mock_chain = builder.build()?;

    // STEP 7: Bob consumes both notes with input_amount = 0 (inflight swap)
    println!("\nBob consuming both swap notes with inflight logic (input_amount = 0)...");

    // Note args for both notes: input_amount = 0
    let alice_note_args = Word::from([Felt::ZERO, Felt::ZERO, Felt::new(50), Felt::ZERO]);
    let charlie_note_args = Word::from([Felt::ZERO, Felt::ZERO, Felt::new(25), Felt::ZERO]);

    let mut note_args_map = BTreeMap::new();
    note_args_map.insert(alice_swap_note.id(), alice_note_args);
    println!("Alice note args: {:?}", alice_swap_note.serial_num());

    note_args_map.insert(charlie_swap_note.id(), charlie_note_args);
    println!("Charlie note args: {:?}", charlie_swap_note.serial_num());

    // STEP 8: Create expected P2ID notes

    // P2ID note for Alice (50 USDC from Charlie's note)
    println!("\nCreating expected P2ID note for Alice (50 USDC)...");
    let alice_p2id_serial_num = Word::from([
        alice_swap_note.recipient().serial_num()[0] + Felt::new(1),
        alice_swap_note.recipient().serial_num()[1] + Felt::new(1),
        alice_swap_note.recipient().serial_num()[2] + Felt::new(1),
        alice_swap_note.recipient().serial_num()[3] + Felt::new(1),
    ]);

    let alice_p2id_recipient = build_p2id_recipient(alice.id(), alice_p2id_serial_num)?;
    let alice_p2id_tag = compute_p2id_tag_for_local_account(alice.id());
    let alice_p2id_aux = Felt::new(50); // 50 USDC
    let alice_p2id_asset = FungibleAsset::new(usdc_faucet.id(), 50)?;
    let alice_p2id_note_assets = NoteAssets::new(vec![alice_p2id_asset.into()])?;

    // Attach aux value to Alice's P2ID note
    let aux_word = Word::from([alice_p2id_aux, Felt::ZERO, Felt::ZERO, Felt::ZERO]);
    let attachment = NoteAttachment::new_word(NoteAttachmentScheme::none(), aux_word);
    let alice_p2id_note_metadata =
        NoteMetadata::new(bob.id(), NoteType::Public, alice_p2id_tag).with_attachment(attachment);
    let alice_p2id_note = Note::new(
        alice_p2id_note_assets,
        alice_p2id_note_metadata,
        alice_p2id_recipient,
    );

    // P2ID note for Charlie (25 ETH from Alice's note)
    println!("\nCreating expected P2ID note for Charlie (25 ETH)...");
    let charlie_p2id_serial_num = Word::from([
        charlie_swap_note.recipient().serial_num()[0] + Felt::new(1),
        charlie_swap_note.recipient().serial_num()[1] + Felt::new(1),
        charlie_swap_note.recipient().serial_num()[2] + Felt::new(1),
        charlie_swap_note.recipient().serial_num()[3] + Felt::new(1),
    ]);

    let charlie_p2id_recipient = build_p2id_recipient(charlie.id(), charlie_p2id_serial_num)?;
    let charlie_p2id_tag = compute_p2id_tag_for_local_account(charlie.id());
    let charlie_p2id_aux = Felt::new(25); // 25 ETH
    let charlie_p2id_asset = FungibleAsset::new(eth_faucet.id(), 25)?;
    let charlie_p2id_note_assets = NoteAssets::new(vec![charlie_p2id_asset.into()])?;

    // Attach aux value to Charlie's P2ID note
    let aux_word = Word::from([charlie_p2id_aux, Felt::ZERO, Felt::ZERO, Felt::ZERO]);
    let attachment = NoteAttachment::new_word(NoteAttachmentScheme::none(), aux_word);
    let charlie_p2id_note_metadata =
        NoteMetadata::new(bob.id(), NoteType::Public, charlie_p2id_tag).with_attachment(attachment);
    let charlie_p2id_note = Note::new(
        charlie_p2id_note_assets,
        charlie_p2id_note_metadata,
        charlie_p2id_recipient,
    );

    // Execute transaction with both notes
    let tx_context = mock_chain
        .build_tx_context(
            bob.id(),
            &[alice_swap_note.id(), charlie_swap_note.id()],
            &[],
        )?
        .extend_expected_output_notes(vec![
            OutputNote::Full(alice_p2id_note),
            OutputNote::Full(charlie_p2id_note),
        ])
        .extend_note_args(note_args_map)
        .build()?;

    let executed_transaction = tx_context.execute().await?;
    println!("Transaction executed successfully!");

    println!(
        "Cycle count: {:?}",
        executed_transaction.measurements().note_execution
    );

    // STEP 9: Verify results
    println!("\n=== Verification ===");

    // Should have exactly 2 P2ID notes, 0 remainder notes
    let output_notes = executed_transaction.output_notes();
    println!("Output notes created: {}", output_notes.num_notes());
    assert_eq!(
        output_notes.num_notes(),
        2,
        "Expected exactly 2 P2ID notes (no remainder notes)"
    );

    // Verify the two P2ID notes
    let mut alice_p2id_found = false;
    let mut charlie_p2id_found = false;

    for idx in 0..output_notes.num_notes() {
        let note = output_notes.get_note(idx);
        println!("\nNote {}: {:?}", idx, note.id());

        let assets = note.assets().unwrap();
        println!("  Number of assets: {}", assets.num_assets());

        if assets.num_assets() == 1 {
            let asset = assets.iter().next().unwrap();
            if let Asset::Fungible(f) = asset {
                println!("  Asset: {} of faucet {:?}", f.amount(), f.faucet_id());

                if f.faucet_id() == usdc_faucet.id() {
                    // This is Alice's P2ID note (50 USDC)
                    assert_eq!(f.amount(), 50, "Alice's P2ID note should contain 50 USDC");
                    println!("  ✓ Alice's P2ID note verified: 50 USDC");
                    alice_p2id_found = true;
                } else if f.faucet_id() == eth_faucet.id() {
                    // This is Charlie's P2ID note (25 ETH)
                    assert_eq!(f.amount(), 25, "Charlie's P2ID note should contain 25 ETH");
                    println!("  ✓ Charlie's P2ID note verified: 25 ETH");
                    charlie_p2id_found = true;
                }
            }
        }
    }

    assert!(alice_p2id_found, "Alice's P2ID note not found");
    assert!(charlie_p2id_found, "Charlie's P2ID note not found");

    // Check Bob's vault delta - should be ZERO (no change)
    println!("\nVerifying Bob's vault delta...");
    let account_delta = executed_transaction.account_delta();
    let vault_delta = account_delta.vault();
    let added_assets: Vec<Asset> = vault_delta.added_assets().collect();
    let removed_assets: Vec<Asset> = vault_delta.removed_assets().collect();

    println!("  Assets added: {}", added_assets.len());
    println!("  Assets removed: {}", removed_assets.len());

    assert_eq!(
        added_assets.len(),
        0,
        "Bob should not receive any assets (inflight swap)"
    );
    assert_eq!(
        removed_assets.len(),
        0,
        "Bob should not spend any assets (inflight swap)"
    );
    println!("  ✓ Bob's vault unchanged (0 added, 0 removed)");

    println!("\n✅ Inflight cross-swap test passed!");
    println!("  - Alice's 25 ETH → Charlie (via P2ID note)");
    println!("  - Charlie's 50 USDC → Alice (via P2ID note)");
    println!("  - Bob facilitated the swap with 0 assets");
    println!("  - 2 P2ID notes created, 0 remainder notes");

    Ok(())
}

#[tokio::test]
async fn swapp_note_inflight_cross_swap_with_spread_test() -> anyhow::Result<()> {
    println!("=== Test: Inflight Cross Swap With Spread (Bob Earns 5 ETH = 2 + 3) ===");
    println!("Alice offers 30 ETH for 50 USDC, Charlie offers 50 USDC for 25 ETH");
    println!("Spread: 30 - 25 = 5 ETH split into 2 P2ID notes (2 ETH + 3 ETH) for Bob");
    let mut builder = MockChain::builder();

    // STEP 1: Create faucets in genesis
    println!("Creating USDC and ETH faucets...");
    let usdc_faucet = builder.add_existing_basic_faucet(
        Auth::BasicAuth,
        "USDC",
        1000,
        Some(100), // total_issuance
    )?;
    println!("USDC Faucet: {:?}", usdc_faucet.id());

    let eth_faucet = builder.add_existing_basic_faucet(
        Auth::BasicAuth,
        "ETH",
        1000,
        Some(50), // total_issuance
    )?;
    println!("ETH Faucet: {:?}", eth_faucet.id());

    // STEP 2: Create Alice, Charlie, and Bob accounts
    println!("\nCreating Alice with 30 ETH (wants 50 USDC)...");
    let alice = builder.add_existing_wallet_with_assets(
        Auth::BasicAuth,
        [FungibleAsset::new(eth_faucet.id(), 30)?.into()],
    )?;
    println!("Alice: {:?} (has 30 ETH)", alice.id());

    println!("\nCreating Charlie with 50 USDC (wants 25 ETH)...");
    let charlie = builder.add_existing_wallet_with_assets(
        Auth::BasicAuth,
        [FungibleAsset::new(usdc_faucet.id(), 50)?.into()],
    )?;
    println!("Charlie: {:?} (has 50 USDC)", charlie.id());

    // Build basic-wallet contract package
    println!("\nBuilding basic-wallet contract...");
    let account_package = Arc::new(build_project_in_dir(
        Path::new("../contracts/basic-wallet"),
        true,
    )?);
    println!("Basic-wallet contract built successfully.");

    // Create Bob's custom account with 0 assets
    println!("\nCreating Bob with 0 assets (custom account)...");
    let bob_account_cfg = AccountCreationConfig {
        storage_slots: vec![],
        ..Default::default()
    };

    let bob = create_testing_account_from_package(
        account_package.clone(),
        bob_account_cfg,
        vec![], // 0 assets
    )
    .await?;
    println!("Bob account created: {:?}", bob.id());

    builder.add_account(bob.clone())?;

    // STEP 3: Build swapp-note contract
    println!("\nBuilding swapp-note contract...");
    let swapp_package = Arc::new(build_project_in_dir(
        Path::new("../contracts/swapp-note"),
        true,
    )?);
    println!("Swapp note contract built successfully.");

    // Build consume-asset-script (creates Bob's spread P2ID note)
    println!("\nBuilding consume-asset-script...");
    let consume_asset_package = Arc::new(build_project_in_dir(
        Path::new("../contracts/consume-asset-script"),
        true,
    )?);
    let program = consume_asset_package.unwrap_program();
    let tx_script =
        TransactionScript::from_parts(program.mast_forest().clone(), program.entrypoint());
    println!("consume-asset-script built successfully.");

    // STEP 4: Create Alice's swap note (offers 30 ETH, wants 50 USDC)
    println!("\nCreating Alice's swap note (offers 30 ETH for 50 USDC)...");

    // Compute proper P2ID tag for Alice (who will receive the output note)
    let alice_p2id_tag_felt = compute_p2id_tag_felt(alice.id());

    let alice_note_inputs = vec![
        // Requested Asset: 50 USDC
        usdc_faucet.id().prefix().into(),
        usdc_faucet.id().suffix(),
        Felt::ZERO,
        Felt::new(50), // requested_asset_total
        // Note Creator: Alice
        alice.id().prefix().into(),
        alice.id().suffix(),
        NoteType::Public.into(),
        // P2ID Tag (position 7): computed tag for Alice
        alice_p2id_tag_felt,
    ];

    let alice_offered_asset = FungibleAsset::new(eth_faucet.id(), 30)?;
    let mut alice_note_assets = NoteAssets::default();
    alice_note_assets.add_asset(alice_offered_asset.into())?;

    let alice_swap_note = create_testing_note_from_package(
        swapp_package.clone(),
        alice.id(),
        NoteCreationConfig {
            assets: alice_note_assets,
            inputs: alice_note_inputs,
            ..Default::default()
        },
    )?;
    println!("Alice's swap note created: {:?}", alice_swap_note.id());

    builder.add_output_note(OutputNote::Full(alice_swap_note.clone()));

    // STEP 5: Create Charlie's swap note (offers 50 USDC, wants 25 ETH)
    println!("\nCreating Charlie's swap note (offers 50 USDC for 25 ETH)...");

    // Compute proper P2ID tag for Charlie (who will receive the output note)
    let charlie_p2id_tag_felt = compute_p2id_tag_felt(charlie.id());

    let charlie_note_inputs = vec![
        // Requested Asset: 25 ETH
        eth_faucet.id().prefix().into(),
        eth_faucet.id().suffix(),
        Felt::ZERO,
        Felt::new(25), // requested_asset_total
        // Note Creator: Charlie
        charlie.id().prefix().into(),
        charlie.id().suffix(),
        NoteType::Public.into(),
        // P2ID Tag (position 7): computed tag for Charlie
        charlie_p2id_tag_felt,
    ];

    let charlie_offered_asset = FungibleAsset::new(usdc_faucet.id(), 50)?;
    let mut charlie_note_assets = NoteAssets::default();
    charlie_note_assets.add_asset(charlie_offered_asset.into())?;

    let charlie_swap_note = create_testing_note_from_package(
        swapp_package.clone(),
        charlie.id(),
        NoteCreationConfig {
            assets: charlie_note_assets,
            inputs: charlie_note_inputs,
            ..Default::default()
        },
    )?;
    println!("Charlie's swap note created: {:?}", charlie_swap_note.id());

    builder.add_output_note(OutputNote::Full(charlie_swap_note.clone()));

    // STEP 6: Build MockChain
    println!("\nBuilding MockChain...");
    let mock_chain = builder.build()?;

    // STEP 7: Bob consumes both notes with input_amount = 0 (inflight swap)
    println!("\nBob consuming both swap notes with inflight logic (input_amount = 0)...");

    // Note args: arg[0]=input, arg[1]=inflight (swapp-note only reads these two)
    // Alice's note: input=0, inflight=50 USDC
    let alice_note_args = Word::from([Felt::ZERO, Felt::ZERO, Felt::new(50), Felt::ZERO]);
    // Charlie's note: input=0, inflight=25 ETH
    let charlie_note_args = Word::from([Felt::ZERO, Felt::ZERO, Felt::new(25), Felt::ZERO]);

    let mut note_args_map = BTreeMap::new();
    note_args_map.insert(alice_swap_note.id(), alice_note_args);
    println!("Alice note args: {:?}", alice_swap_note.serial_num());

    note_args_map.insert(charlie_swap_note.id(), charlie_note_args);
    println!("Charlie note args: {:?}", charlie_swap_note.serial_num());

    // STEP 8: Create expected P2ID notes

    // P2ID note for Alice (50 USDC from Charlie's note)
    println!("\nCreating expected P2ID note for Alice (50 USDC)...");
    let alice_p2id_serial_num = Word::from([
        alice_swap_note.recipient().serial_num()[0] + Felt::new(1),
        alice_swap_note.recipient().serial_num()[1] + Felt::new(1),
        alice_swap_note.recipient().serial_num()[2] + Felt::new(1),
        alice_swap_note.recipient().serial_num()[3] + Felt::new(1),
    ]);

    let alice_p2id_recipient = build_p2id_recipient(alice.id(), alice_p2id_serial_num)?;
    let alice_p2id_tag = compute_p2id_tag_for_local_account(alice.id());
    let alice_p2id_aux = Felt::new(50); // 50 USDC
    let alice_p2id_asset = FungibleAsset::new(usdc_faucet.id(), 50)?;
    let alice_p2id_note_assets = NoteAssets::new(vec![alice_p2id_asset.into()])?;

    // Attach aux value to Alice's P2ID note
    let aux_word = Word::from([alice_p2id_aux, Felt::ZERO, Felt::ZERO, Felt::ZERO]);
    let attachment = NoteAttachment::new_word(NoteAttachmentScheme::none(), aux_word);
    let alice_p2id_note_metadata =
        NoteMetadata::new(bob.id(), NoteType::Public, alice_p2id_tag).with_attachment(attachment);
    let alice_p2id_note = Note::new(
        alice_p2id_note_assets,
        alice_p2id_note_metadata,
        alice_p2id_recipient,
    );

    // P2ID note for Charlie (25 ETH from Alice's note)
    println!("\nCreating expected P2ID note for Charlie (25 ETH)...");
    let charlie_p2id_serial_num = Word::from([
        charlie_swap_note.recipient().serial_num()[0] + Felt::new(1),
        charlie_swap_note.recipient().serial_num()[1] + Felt::new(1),
        charlie_swap_note.recipient().serial_num()[2] + Felt::new(1),
        charlie_swap_note.recipient().serial_num()[3] + Felt::new(1),
    ]);

    let charlie_p2id_recipient = build_p2id_recipient(charlie.id(), charlie_p2id_serial_num)?;
    let charlie_p2id_tag = compute_p2id_tag_for_local_account(charlie.id());
    let charlie_p2id_aux = Felt::new(25); // 25 ETH
    let charlie_p2id_asset = FungibleAsset::new(eth_faucet.id(), 25)?;
    let charlie_p2id_note_assets = NoteAssets::new(vec![charlie_p2id_asset.into()])?;

    // Attach aux value to Charlie's P2ID note
    let aux_word = Word::from([charlie_p2id_aux, Felt::ZERO, Felt::ZERO, Felt::ZERO]);
    let attachment = NoteAttachment::new_word(NoteAttachmentScheme::none(), aux_word);
    let charlie_p2id_note_metadata =
        NoteMetadata::new(bob.id(), NoteType::Public, charlie_p2id_tag).with_attachment(attachment);
    let charlie_p2id_note = Note::new(
        charlie_p2id_note_assets,
        charlie_p2id_note_metadata,
        charlie_p2id_recipient,
    );

    // Build advice stack for consume-asset-script (2 spread notes: 2 ETH + 3 ETH)
    println!("\nBuilding advice stack for Bob's P2ID tx-script (2 notes)...");
    let bob_asset_word1 = Word::from(Asset::from(FungibleAsset::new(eth_faucet.id(), 2)?));
    let bob_asset_word2 = Word::from(Asset::from(FungibleAsset::new(eth_faucet.id(), 3)?));

    let advice_stack: Vec<Felt> = vec![
        // asset 1
        bob_asset_word1[0],
        bob_asset_word1[1],
        bob_asset_word1[2],
        bob_asset_word1[3],
        // asset 2
        bob_asset_word2[0],
        bob_asset_word2[1],
        bob_asset_word2[2],
        bob_asset_word2[3],
    ];

    let commitment_key: Word = Rpo256::hash_elements(&advice_stack);
    let mut commitment = commitment_key;
    commitment.reverse();

    // Execute transaction with both notes + consume-asset-script for Bob's spread (2 notes)
    // 4 P2ID notes: Alice(50 USDC), Charlie(25 ETH), Bob(2 ETH), Bob(3 ETH)
    let tx_context = mock_chain
        .build_tx_context(
            bob.id(),
            &[alice_swap_note.id(), charlie_swap_note.id()],
            &[],
        )?
        .tx_script(tx_script)
        .tx_script_args(commitment)
        .extend_advice_map([(commitment_key, advice_stack)])
        .extend_expected_output_notes(vec![
            OutputNote::Full(alice_p2id_note),
            OutputNote::Full(charlie_p2id_note),
        ])
        .extend_note_args(note_args_map)
        .build()?;

    let executed_transaction = tx_context.execute().await?;
    println!("Transaction executed successfully!");

    println!(
        "Cycle count: {:?}",
        executed_transaction.measurements().note_execution
    );

    // STEP 9: Verify results
    println!("\n=== Verification ===");

    // Should have exactly 2 P2ID notes (Alice, Charlie) - Bob consumes spread directly
    let output_notes = executed_transaction.output_notes();
    println!("Output notes created: {}", output_notes.num_notes());
    assert_eq!(
        output_notes.num_notes(),
        2,
        "Expected exactly 2 P2ID notes (Alice 50 USDC, Charlie 25 ETH)"
    );

    // Verify the two P2ID notes
    let mut alice_p2id_found = false;
    let mut charlie_p2id_found = false;

    for idx in 0..output_notes.num_notes() {
        let note = output_notes.get_note(idx);
        println!("\nNote {}: {:?}", idx, note.id());

        let assets = note.assets().unwrap();
        println!("  Number of assets: {}", assets.num_assets());

        if assets.num_assets() == 1 {
            let asset = assets.iter().next().unwrap();
            if let Asset::Fungible(f) = asset {
                println!("  Asset: {} of faucet {:?}", f.amount(), f.faucet_id());

                if f.faucet_id() == usdc_faucet.id() && f.amount() == 50 {
                    println!("  -> Alice's P2ID note verified: 50 USDC");
                    alice_p2id_found = true;
                } else if f.faucet_id() == eth_faucet.id() && f.amount() == 25 {
                    println!("  -> Charlie's P2ID note verified: 25 ETH");
                    charlie_p2id_found = true;
                }
            }
        }
    }

    assert!(alice_p2id_found, "Alice's P2ID note (50 USDC) not found");
    assert!(charlie_p2id_found, "Charlie's P2ID note (25 ETH) not found");

    // Check Bob's vault delta - 5 ETH spread should be added directly to Bob's vault
    println!("\nVerifying Bob's vault delta...");
    let account_delta = executed_transaction.account_delta();
    let vault_delta = account_delta.vault();
    let added_assets: Vec<Asset> = vault_delta.added_assets().collect();
    let removed_assets: Vec<Asset> = vault_delta.removed_assets().collect();

    println!("  Assets added: {}", added_assets.len());
    println!("  Assets removed: {}", removed_assets.len());

    assert_eq!(removed_assets.len(), 0, "Bob should not spend any assets");
    assert_eq!(added_assets.len(), 1, "Bob should receive 1 asset (5 ETH spread)");
    if let Asset::Fungible(f) = &added_assets[0] {
        assert_eq!(f.faucet_id(), eth_faucet.id(), "Added asset should be ETH");
        assert_eq!(f.amount(), 5, "Bob should receive 5 ETH spread");
        println!("  -> Bob's vault verified: +5 ETH");
    } else {
        panic!("Expected fungible asset in Bob's vault");
    }

    println!("\n=== Inflight cross-swap with spread test passed! ===");
    println!("  - Alice offered 30 ETH for 50 USDC (fully filled)");
    println!("  - Charlie offered 50 USDC for 25 ETH (fully filled)");
    println!("  - Bob earned 5 ETH spread directly into vault");
    println!("  - 2 P2ID notes: Alice(50 USDC), Charlie(25 ETH)");

    Ok(())
}

#[tokio::test]
async fn swapp_note_creator_reclaim_test() -> anyhow::Result<()> {
    println!("=== Test: Creator Reclaim ===");
    let mut builder = MockChain::builder();

    // STEP 1: Create faucets
    println!("Creating USDC and ETH faucets...");
    let usdc_faucet = builder.add_existing_basic_faucet(Auth::BasicAuth, "USDC", 1000, Some(50))?;

    let eth_faucet = builder.add_existing_basic_faucet(Auth::BasicAuth, "ETH", 1000, Some(25))?;

    // STEP 2: Build basic-wallet contract package
    println!("\nBuilding basic-wallet contract...");
    let account_package = Arc::new(build_project_in_dir(
        Path::new("../contracts/basic-wallet"),
        true,
    )?);
    println!("Basic-wallet contract built successfully.");

    // STEP 3: Create Alice wallet with USDC (using custom wallet)
    println!("\nCreating Alice wallet...");
    let alice_account_cfg = AccountCreationConfig {
        storage_slots: vec![],
        ..Default::default()
    };

    let alice_assets = vec![FungibleAsset::new(usdc_faucet.id(), 50)?.into()];

    let alice = create_testing_account_from_package(
        account_package.clone(),
        alice_account_cfg,
        alice_assets,
    )
    .await?;
    println!("Alice: {:?}", alice.id());

    let _alice_account = builder.add_account(alice.clone());

    // STEP 4: Build swapp-note contract
    println!("\nBuilding swapp-note contract...");
    let swapp_package = Arc::new(build_project_in_dir(
        Path::new("../contracts/swapp-note"),
        true,
    )?);

    // STEP 5: Create swap note
    println!("\nCreating swap note (Alice offers 50 USDC for 25 ETH)...");

    // Compute proper P2ID tag for Alice (who will receive the output note)
    let p2id_tag_felt = compute_p2id_tag_felt(alice.id());

    let note_inputs = vec![
        // Requested Asset: 25 ETH
        eth_faucet.id().prefix().into(),
        eth_faucet.id().suffix(),
        Felt::ZERO,
        Felt::new(25),
        // Note Creator: Alice
        alice.id().prefix().into(),
        alice.id().suffix(),
        NoteType::Public.into(),
        // P2ID Tag (position 7): computed tag for Alice
        p2id_tag_felt,
    ];

    let offered_asset = FungibleAsset::new(usdc_faucet.id(), 50)?;
    let mut note_assets = NoteAssets::default();
    note_assets.add_asset(offered_asset.into())?;

    let swap_note = create_testing_note_from_package(
        swapp_package.clone(),
        alice.id(),
        NoteCreationConfig {
            assets: note_assets,
            inputs: note_inputs,
            ..Default::default()
        },
    )?;

    builder.add_output_note(OutputNote::Full(swap_note.clone()));

    // STEP 6: Alice reclaims her own note
    println!("\nBuilding MockChain...");
    let mock_chain = builder.build()?;

    println!("\nAlice reclaiming her own swap note (no args needed)...");
    // No note args needed for reclaim - contract detects creator == consumer
    let tx_context = mock_chain
        .build_tx_context(alice.id(), &[swap_note.id()], &[])?
        .build()?;

    let executed_transaction = tx_context.execute().await?;
    println!("Transaction executed successfully!");

    println!(
        "Cycle count: {:?}",
        executed_transaction.measurements().note_execution
    );

    // STEP 7: Verify results
    println!("\n=== Verification ===");

    // Should have NO output notes (no P2ID, no remainder)
    let output_notes = executed_transaction.output_notes();
    println!("Output notes created: {}", output_notes.num_notes());
    assert_eq!(
        output_notes.num_notes(),
        0,
        "Expected 0 output notes for reclaim"
    );
    println!("✓ No output notes created (correct for reclaim)");

    // Check Alice's vault delta - should have received back 50 USDC
    let account_delta = executed_transaction.account_delta();
    let vault_delta = account_delta.vault();
    let added_assets: Vec<Asset> = vault_delta.added_assets().collect();

    assert_eq!(added_assets.len(), 1, "Alice should receive 1 asset back");
    let usdc_reclaimed = match added_assets[0] {
        Asset::Fungible(f) => f,
        _ => panic!("Expected fungible USDC asset"),
    };
    assert_eq!(
        usdc_reclaimed.faucet_id(),
        usdc_faucet.id(),
        "Alice should reclaim USDC"
    );
    assert_eq!(usdc_reclaimed.amount(), 50, "Alice should reclaim 50 USDC");
    println!("✓ Alice's vault delta verified: +50 USDC");

    println!("\n✅ Creator reclaim test passed!");
    println!("  - Alice reclaimed her own swap note");
    println!("  - Received back 50 USDC");
    println!("  - No P2ID or remainder notes created");

    Ok(())
}

#[tokio::test]
async fn swapp_note_invalid_input_test() -> anyhow::Result<()> {
    println!("=== Test: Invalid Input (Requesting More Than Available) ===");
    let mut builder = MockChain::builder();

    // STEP 1: Create faucets
    let usdc_faucet = builder.add_existing_basic_faucet(Auth::BasicAuth, "USDC", 1000, Some(50))?;

    let eth_faucet = builder.add_existing_basic_faucet(
        Auth::BasicAuth,
        "ETH",
        1000,
        Some(30), // Bob has 30 ETH
    )?;

    // STEP 2: Create wallets
    let alice = builder.add_existing_wallet_with_assets(
        Auth::BasicAuth,
        [FungibleAsset::new(usdc_faucet.id(), 50)?.into()],
    )?;

    let bob = builder.add_existing_wallet_with_assets(
        Auth::BasicAuth,
        [FungibleAsset::new(eth_faucet.id(), 30)?.into()],
    )?;

    // STEP 3: Build contract
    let swapp_package = Arc::new(build_project_in_dir(
        Path::new("../contracts/swapp-note"),
        true,
    )?);

    // STEP 4: Create swap note (Alice wants 25 ETH max)
    // Compute proper P2ID tag for Alice (who will receive the output note)
    let p2id_tag_felt = compute_p2id_tag_felt(alice.id());

    let note_inputs = vec![
        eth_faucet.id().prefix().into(),
        eth_faucet.id().suffix(),
        Felt::ZERO,
        Felt::new(25), // requested_asset_total = 25
        alice.id().prefix().into(),
        alice.id().suffix(),
        NoteType::Public.into(),
        // P2ID Tag (position 7): computed tag for Alice
        p2id_tag_felt,
    ];

    let offered_asset = FungibleAsset::new(usdc_faucet.id(), 50)?;
    let mut note_assets = NoteAssets::default();
    note_assets.add_asset(offered_asset.into())?;

    let swap_note = create_testing_note_from_package(
        swapp_package.clone(),
        alice.id(),
        NoteCreationConfig {
            assets: note_assets,
            inputs: note_inputs,
            ..Default::default()
        },
    )?;

    builder.add_output_note(OutputNote::Full(swap_note.clone()));

    let mock_chain = builder.build()?;

    // STEP 5: Bob tries to provide MORE than requested (30 > 25) - should fail
    println!("\nBob trying to provide 30 ETH (more than requested 25)...");
    let note_args = Word::from([Felt::ZERO, Felt::ZERO, Felt::ZERO, Felt::new(30)]);

    let mut note_args_map = BTreeMap::new();
    note_args_map.insert(swap_note.id(), note_args);

    let tx_context = mock_chain
        .build_tx_context(bob.id(), &[swap_note.id()], &[])?
        .extend_note_args(note_args_map)
        .build()?;

    // This should fail with assertion error
    let result = tx_context.execute().await;

    assert!(
        result.is_err(),
        "Transaction should fail when input_amount > requested_asset_total"
    );

    println!("✓ Transaction correctly failed with assertion error");
    println!("\n✅ Invalid input test passed!");
    println!("  - Bob tried to provide 30 ETH (more than 25 requested)");
    println!("  - Transaction failed as expected (assertion at line 75)");

    Ok(())
}

/// Fuzz test: runs many inflight cross-swap scenarios with different amounts
/// to verify the contract works correctly with arbitrary numbers.
///
/// Each case: Alice offers `alice_eth` ETH for `usdc` USDC,
///            Charlie offers `usdc` USDC for `charlie_eth` ETH.
/// Bob facilitates the inflight swap and earns `alice_eth - charlie_eth` ETH spread.
///
/// Constraints:
///   - alice_eth > charlie_eth  (positive spread for Bob)
///   - charlie_offered_usdc == alice_requested_usdc  (exact match for full fills)
///   - All amounts >= 1
#[tokio::test]
async fn swapp_note_inflight_cross_swap_fuzz_test() -> anyhow::Result<()> {
    println!("=== Fuzz Test: Inflight Cross Swap With Spread ===\n");

    // Build contracts once (expensive)
    println!("Building contracts (one-time setup)...");
    let account_package = Arc::new(build_project_in_dir(
        Path::new("../contracts/basic-wallet"),
        true,
    )?);
    let consume_asset_package = Arc::new(build_project_in_dir(
        Path::new("../contracts/consume-asset-script"),
        true,
    )?);
    println!("Contracts built.\n");

    // Handpicked edge cases: (alice_offered_eth, alice_requested_usdc, charlie_requested_eth)
    let mut test_cases: Vec<(u64, u64, u64)> = vec![
        // Minimum amounts
        (2, 1, 1),   // spread=1, smallest possible
        (3, 1, 1),   // spread=2, tiny usdc
        (2, 100, 1), // spread=1, tiny eth but large usdc
        // Spread = 1 (tightest margin)
        (10, 10, 9),   // equal offered/requested
        (50, 1, 49),   // large amounts, tiny usdc
        (100, 99, 99), // large amounts
        // Large spread
        (100, 100, 1), // spread=99, almost all spread
        (99, 50, 1),   // spread=98
        // Primes (non-divisible, stress integer arithmetic)
        (7, 5, 3),    // tiny primes, spread=4
        (17, 13, 11), // small primes, spread=6
        (53, 47, 41), // medium primes, spread=12
        (97, 89, 83), // large primes, spread=14
        // Odd/even mixes
        (23, 21, 17), // all odd, spread=6
        (24, 21, 17), // even/odd/odd, spread=7
        (23, 22, 17), // odd/even/odd, spread=6
        // Close consecutive numbers
        (51, 49, 47), // close odds, spread=4
        (4, 3, 2),    // consecutive, spread=2
        // Powers of 2
        (64, 32, 16),  // powers of 2, spread=48
        (128, 64, 32), // larger powers, spread=96
        // Same offered/requested ratio stress
        (10, 10, 5),  // 1:1 offer ratio, spread=5
        (20, 10, 15), // 2:1, spread=5
        (10, 20, 5),  // 1:2, spread=5
        // Duplicate amount edge case (charlie_eth == spread)
        (4, 5, 2),  // charlie=2, spread=2 (duplicate!)
        (6, 10, 3), // charlie=3, spread=3 (duplicate!)
    ];

    // Add random cases
    use rand::Rng;
    let mut thread_rng = rand::rng();
    for _ in 0..20 {
        let alice_eth: u64 = thread_rng.random_range(2..500);
        let usdc: u64 = thread_rng.random_range(1..500);
        let charlie_eth: u64 = thread_rng.random_range(1..alice_eth);
        test_cases.push((alice_eth, usdc, charlie_eth));
    }

    let total = test_cases.len();
    println!(
        "Running {} test cases ({} handpicked + 20 random)...\n",
        total,
        total - 20
    );

    for (i, (alice_eth, usdc, charlie_eth)) in test_cases.iter().enumerate() {
        let spread = alice_eth - charlie_eth;
        println!(
            "--- Case {}/{}: Alice offers {} ETH for {} USDC | Charlie offers {} USDC for {} ETH | spread={} ETH ---",
            i + 1, total, alice_eth, usdc, usdc, charlie_eth, spread
        );

        // Fresh MockChain per case
        let mut builder = MockChain::builder();

        let max_amount = *[*alice_eth, *usdc].iter().max().unwrap();
        let total_issuance = max_amount * 10;

        let usdc_faucet = builder.add_existing_basic_faucet(
            Auth::BasicAuth,
            "USDC",
            1000,
            Some(total_issuance),
        )?;
        let eth_faucet = builder.add_existing_basic_faucet(
            Auth::BasicAuth,
            "ETH",
            1000,
            Some(total_issuance),
        )?;

        let alice = builder.add_existing_wallet_with_assets(
            Auth::BasicAuth,
            [FungibleAsset::new(eth_faucet.id(), *alice_eth)?.into()],
        )?;
        let charlie = builder.add_existing_wallet_with_assets(
            Auth::BasicAuth,
            [FungibleAsset::new(usdc_faucet.id(), *usdc)?.into()],
        )?;

        let bob = create_testing_account_from_package(
            account_package.clone(),
            AccountCreationConfig {
                storage_slots: vec![],
                ..Default::default()
            },
            vec![],
        )
        .await?;
        builder.add_account(bob.clone())?;

        // Create swap notes using PswapNote::create()
        let seed_word = Word::from([
            Felt::new(i as u64 + 1),
            Felt::new(i as u64 + 2),
            Felt::new(i as u64 + 3),
            Felt::new(i as u64 + 4),
        ]);
        let mut note_rng = miden_crypto::rand::RpoRandomCoin::new(seed_word);

        // Alice's swap note: offers alice_eth ETH, wants usdc USDC
        let alice_swap_note = create_swapp_note_with_pswap(
            alice.id(),
            Asset::Fungible(FungibleAsset::new(eth_faucet.id(), *alice_eth)?),
            Asset::Fungible(FungibleAsset::new(usdc_faucet.id(), *usdc)?),
            NoteType::Public,
            &mut note_rng,
        )?;
        builder.add_output_note(OutputNote::Full(alice_swap_note.clone()));

        // Charlie's swap note: offers usdc USDC, wants charlie_eth ETH
        let charlie_swap_note = create_swapp_note_with_pswap(
            charlie.id(),
            Asset::Fungible(FungibleAsset::new(usdc_faucet.id(), *usdc)?),
            Asset::Fungible(FungibleAsset::new(eth_faucet.id(), *charlie_eth)?),
            NoteType::Public,
            &mut note_rng,
        )?;
        builder.add_output_note(OutputNote::Full(charlie_swap_note.clone()));

        // Build tx_script from shared package
        let program = consume_asset_package.unwrap_program();
        let tx_script =
            TransactionScript::from_parts(program.mast_forest().clone(), program.entrypoint());

        let mock_chain = builder.build()?;

        // Note args: input=0, inflight=fill_amount
        let alice_note_args = Word::from([Felt::ZERO, Felt::ZERO, Felt::new(*usdc), Felt::ZERO]);
        let charlie_note_args =
            Word::from([Felt::ZERO, Felt::ZERO, Felt::new(*charlie_eth), Felt::ZERO]);
        let mut note_args_map = BTreeMap::new();
        note_args_map.insert(alice_swap_note.id(), alice_note_args);
        note_args_map.insert(charlie_swap_note.id(), charlie_note_args);

        // Expected P2ID notes using PswapNote::create_output_notes()
        let (alice_p2id_note, alice_rem) =
            PswapNote::create_output_notes(&alice_swap_note, bob.id(), 0, *usdc)?;
        assert!(
            alice_rem.is_none(),
            "Case {}: Alice full fill should not produce remainder",
            i + 1
        );

        let (charlie_p2id_note, charlie_rem) =
            PswapNote::create_output_notes(&charlie_swap_note, bob.id(), 0, *charlie_eth)?;
        assert!(
            charlie_rem.is_none(),
            "Case {}: Charlie full fill should not produce remainder",
            i + 1
        );

        // Advice stack for spread (assets consumed directly into Bob's vault)
        let bob_asset_word = Word::from(Asset::from(FungibleAsset::new(eth_faucet.id(), spread)?));
        let advice_stack: Vec<Felt> = vec![
            bob_asset_word[0],
            bob_asset_word[1],
            bob_asset_word[2],
            bob_asset_word[3],
        ];

        let commitment_key: Word = Rpo256::hash_elements(&advice_stack);
        let mut commitment = commitment_key;
        commitment.reverse();

        // Save expected note IDs and assets before notes are moved
        let expected_note_checks = vec![
            (alice_p2id_note.id(), usdc_faucet.id(), *usdc, "Alice P2ID"),
            (
                charlie_p2id_note.id(),
                eth_faucet.id(),
                *charlie_eth,
                "Charlie P2ID",
            ),
        ];

        // Execute transaction
        let tx_context = mock_chain
            .build_tx_context(
                bob.id(),
                &[alice_swap_note.id(), charlie_swap_note.id()],
                &[],
            )?
            .tx_script(tx_script)
            .tx_script_args(commitment)
            .extend_advice_map([(commitment_key, advice_stack)])
            .extend_expected_output_notes(vec![
                OutputNote::Full(alice_p2id_note),
                OutputNote::Full(charlie_p2id_note),
            ])
            .extend_note_args(note_args_map)
            .build()?;

        let executed_tx = tx_context.execute().await?;

        // === Verify ===
        let output_notes = executed_tx.output_notes();
        assert_eq!(
            output_notes.num_notes(),
            2,
            "Case {}: Expected 2 P2ID notes, got {}",
            i + 1,
            output_notes.num_notes()
        );

        // Per-note asset verification: check each expected note's faucet_id and amount
        for (expected_id, expected_faucet, expected_amount, label) in &expected_note_checks {
            let mut found = false;
            for idx in 0..output_notes.num_notes() {
                let note = output_notes.get_note(idx);
                if note.id() == *expected_id {
                    let assets = note.assets().unwrap();
                    assert_eq!(
                        assets.num_assets(),
                        1,
                        "Case {}: {} should have exactly 1 asset",
                        i + 1,
                        label
                    );
                    if let Asset::Fungible(f) = assets.iter().next().unwrap() {
                        assert_eq!(
                            f.faucet_id(),
                            *expected_faucet,
                            "Case {}: {} faucet_id mismatch",
                            i + 1,
                            label
                        );
                        assert_eq!(
                            f.amount(),
                            *expected_amount,
                            "Case {}: {} amount mismatch (expected {}, got {})",
                            i + 1,
                            label,
                            expected_amount,
                            f.amount()
                        );
                    } else {
                        panic!("Case {}: {} asset is not fungible", i + 1, label);
                    }
                    found = true;
                    break;
                }
            }
            assert!(found, "Case {}: {} note not found in output", i + 1, label);
        }

        // Bob's vault should have the spread added directly
        let vault_delta = executed_tx.account_delta().vault();
        assert_eq!(
            vault_delta.removed_assets().count(),
            0,
            "Case {}: Bob should not spend any assets",
            i + 1
        );
        let added_assets: Vec<Asset> = vault_delta.added_assets().collect();
        assert_eq!(
            added_assets.len(),
            1,
            "Case {}: Bob should receive 1 asset ({} ETH spread)",
            i + 1,
            spread
        );
        if let Asset::Fungible(f) = &added_assets[0] {
            assert_eq!(f.faucet_id(), eth_faucet.id(), "Case {}: Added asset should be ETH", i + 1);
            assert_eq!(f.amount(), spread, "Case {}: Bob should receive {} ETH spread", i + 1, spread);
        } else {
            panic!("Case {}: Expected fungible asset in Bob's vault", i + 1);
        }

        println!(
            "  PASSED: Alice {} USDC, Charlie {} ETH, Bob {} ETH spread (in vault)",
            usdc, charlie_eth, spread
        );
    }

    println!("\n=== All {} fuzz test cases passed! ===", total);
    Ok(())
}

/// Fuzz test: inflight cross-swap where BOTH notes are partially filled.
///
/// Each case: Alice offers `alice_eth` ETH for `alice_usdc` USDC,
///            Charlie offers `charlie_usdc` USDC for `charlie_eth` ETH.
/// Bob partially fills both notes via inflight and earns spread.
///
/// Expected output per case (5 notes):
///   1. Alice P2ID:      alice_fill_usdc USDC
///   2. Alice remainder:  (alice_eth - alice_offered_out) ETH for (alice_usdc - alice_fill_usdc) USDC
///   3. Charlie P2ID:     charlie_fill_eth ETH
///   4. Charlie remainder: (charlie_usdc - charlie_offered_out) USDC for (charlie_eth - charlie_fill_eth) ETH
///   5. Bob spread P2ID:  (alice_offered_out - charlie_fill_eth) ETH
///
/// To ensure on-chain Felt arithmetic matches Rust integer arithmetic, all cases
/// use ratios where offered is a multiple of requested (exact integer division).
#[tokio::test]
async fn swapp_note_inflight_partial_fill_cross_swap_fuzz_test() -> anyhow::Result<()> {
    println!("=== Fuzz Test: Inflight Partial Fill Cross Swap (Both Remainders + Spread) ===\n");

    // Build contracts once (expensive)
    println!("Building contracts (one-time setup)...");
    let account_package = Arc::new(build_project_in_dir(
        Path::new("../contracts/basic-wallet"),
        true,
    )?);
    let consume_asset_package = Arc::new(build_project_in_dir(
        Path::new("../contracts/consume-asset-script"),
        true,
    )?);
    println!("Contracts built.\n");

    // Handpicked test cases: (alice_eth, alice_usdc, charlie_usdc, charlie_eth, charlie_fill_eth)
    //
    // For each case, the derived values are:
    //   charlie_offered_out = calculate_output_amount(charlie_usdc, charlie_eth, charlie_fill_eth)
    //   alice_fill_usdc     = charlie_offered_out  (inflight routing)
    //   alice_offered_out   = calculate_output_amount(alice_eth, alice_usdc, alice_fill_usdc)
    //   spread              = alice_offered_out - charlie_fill_eth
    //
    // Constraints:
    //   charlie_fill_eth < charlie_eth            (partial fill for Charlie)
    //   alice_fill_usdc  < alice_usdc             (partial fill for Alice)
    //   alice_offered_out > charlie_fill_eth       (positive spread)
    //   remainders > 0 for both notes
    let mut test_cases: Vec<(u64, u64, u64, u64, u64)> = vec![
        // (alice_eth, alice_usdc, charlie_usdc, charlie_eth, charlie_fill_eth)
        //
        // Both notes: offered > requested, ratio is integer
        (100, 50, 60, 30, 15), // 2:1 / 2:1, spread=45
        (10, 5, 8, 4, 2),      // 2:1 / 2:1, spread=6
        (50, 25, 30, 15, 5),   // 2:1 / 2:1, spread=15
        (20, 10, 14, 7, 3),    // 2:1 / 2:1, spread=9
        (6, 3, 4, 2, 1),       // 2:1 / 2:1, spread=3 (tiny)
        (40, 20, 10, 5, 2),    // 2:1 / 2:1, spread=6
        (14, 7, 10, 5, 3),     // 2:1 / 2:1, spread=9
        // Mixed ratios
        (90, 45, 80, 40, 20),    // 2:1 / 2:1, spread=60
        (30, 15, 24, 8, 4),      // 2:1 / 3:1, spread=20
        (200, 100, 150, 50, 25), // 2:1 / 3:1, spread=125
        // Alice's offered <= requested (ratio 1:2 on Alice side)
        (10, 20, 30, 10, 4), // 1:2 / 3:1, spread=2
    ];

    // Generate random cases where both notes have offered > requested with integer ratios
    // This guarantees calculate_output_amount(offered, requested, fill) = fill * (offered/requested)
    use rand::Rng;
    let mut thread_rng = rand::rng();
    for _ in 0..15 {
        let charlie_eth: u64 = thread_rng.random_range(3..50);
        let charlie_ratio: u64 = thread_rng.random_range(2..8);
        let charlie_usdc = charlie_eth * charlie_ratio;
        let charlie_fill_eth: u64 = thread_rng.random_range(1..charlie_eth);

        // charlie_offered_out = charlie_fill_eth * charlie_ratio (exact)
        let charlie_offered_out = charlie_fill_eth * charlie_ratio;
        let alice_fill_usdc = charlie_offered_out;

        // Ensure alice_usdc > alice_fill_usdc (partial fill for Alice)
        let alice_usdc_extra: u64 = thread_rng.random_range(1..50);
        let alice_usdc = alice_fill_usdc + alice_usdc_extra;
        let alice_ratio: u64 = thread_rng.random_range(2..8);
        let alice_eth = alice_usdc * alice_ratio;

        // alice_offered_out = alice_fill_usdc * alice_ratio (exact, always > charlie_fill_eth)
        test_cases.push((
            alice_eth,
            alice_usdc,
            charlie_usdc,
            charlie_eth,
            charlie_fill_eth,
        ));
    }

    let total = test_cases.len();
    println!(
        "Running {} test cases ({} handpicked + 15 random)...\n",
        total,
        total - 15
    );

    for (i, (alice_eth, alice_usdc, charlie_usdc, charlie_eth, charlie_fill_eth)) in
        test_cases.iter().enumerate()
    {
        // Derive values using the same calculation as on-chain
        let charlie_offered_out =
            PswapNote::calculate_output_amount(*charlie_usdc, *charlie_eth, *charlie_fill_eth);
        let alice_fill_usdc = charlie_offered_out;
        let alice_offered_out =
            PswapNote::calculate_output_amount(*alice_eth, *alice_usdc, alice_fill_usdc);

        // Validate constraints
        assert!(
            *charlie_fill_eth < *charlie_eth,
            "Case {}: Charlie fill must be partial",
            i + 1
        );
        assert!(
            alice_fill_usdc < *alice_usdc,
            "Case {}: Alice fill must be partial (alice_fill={}, alice_usdc={})",
            i + 1,
            alice_fill_usdc,
            alice_usdc
        );
        assert!(
            alice_offered_out > *charlie_fill_eth,
            "Case {}: Need positive spread",
            i + 1
        );
        assert!(
            charlie_offered_out > 0,
            "Case {}: Charlie must release USDC",
            i + 1
        );

        let spread = alice_offered_out - charlie_fill_eth;
        let alice_remaining_eth = alice_eth - alice_offered_out;
        let charlie_remaining_usdc = charlie_usdc - charlie_offered_out;
        let alice_remaining_usdc = alice_usdc - alice_fill_usdc;
        let charlie_remaining_eth = charlie_eth - charlie_fill_eth;

        assert!(
            alice_remaining_eth > 0,
            "Case {}: Alice remainder ETH=0",
            i + 1
        );
        assert!(
            charlie_remaining_usdc > 0,
            "Case {}: Charlie remainder USDC=0",
            i + 1
        );

        println!(
            "--- Case {}/{}: Alice {}ETH for {}USDC | Charlie {}USDC for {}ETH | fill={}ETH ---",
            i + 1,
            total,
            alice_eth,
            alice_usdc,
            charlie_usdc,
            charlie_eth,
            charlie_fill_eth
        );
        println!(
            "    derived: charlie_out={} USDC, alice_fill={} USDC, alice_out={} ETH, spread={} ETH",
            charlie_offered_out, alice_fill_usdc, alice_offered_out, spread
        );
        println!(
            "    remainders: Alice {}ETH/{}USDC, Charlie {}USDC/{}ETH",
            alice_remaining_eth,
            alice_remaining_usdc,
            charlie_remaining_usdc,
            charlie_remaining_eth
        );

        // Fresh MockChain per case
        let mut builder = MockChain::builder();

        let max_amount = *[*alice_eth, *alice_usdc, *charlie_usdc, *charlie_eth]
            .iter()
            .max()
            .unwrap();
        let total_issuance = max_amount * 10;

        let usdc_faucet = builder.add_existing_basic_faucet(
            Auth::BasicAuth,
            "USDC",
            1000,
            Some(total_issuance),
        )?;
        let eth_faucet = builder.add_existing_basic_faucet(
            Auth::BasicAuth,
            "ETH",
            1000,
            Some(total_issuance),
        )?;

        let alice = builder.add_existing_wallet_with_assets(
            Auth::BasicAuth,
            [FungibleAsset::new(eth_faucet.id(), *alice_eth)?.into()],
        )?;
        let charlie = builder.add_existing_wallet_with_assets(
            Auth::BasicAuth,
            [FungibleAsset::new(usdc_faucet.id(), *charlie_usdc)?.into()],
        )?;

        let bob = create_testing_account_from_package(
            account_package.clone(),
            AccountCreationConfig {
                storage_slots: vec![],
                ..Default::default()
            },
            vec![],
        )
        .await?;
        builder.add_account(bob.clone())?;

        // Create swap notes using PswapNote::create()
        let seed_word = Word::from([
            Felt::new(i as u64 * 4 + 100),
            Felt::new(i as u64 * 4 + 101),
            Felt::new(i as u64 * 4 + 102),
            Felt::new(i as u64 * 4 + 103),
        ]);
        let mut note_rng = miden_crypto::rand::RpoRandomCoin::new(seed_word);

        // Alice's swap note: offers alice_eth ETH, wants alice_usdc USDC
        let alice_swap_note = create_swapp_note_with_pswap(
            alice.id(),
            Asset::Fungible(FungibleAsset::new(eth_faucet.id(), *alice_eth)?),
            Asset::Fungible(FungibleAsset::new(usdc_faucet.id(), *alice_usdc)?),
            NoteType::Public,
            &mut note_rng,
        )?;
        builder.add_output_note(OutputNote::Full(alice_swap_note.clone()));

        // Charlie's swap note: offers charlie_usdc USDC, wants charlie_eth ETH
        let charlie_swap_note = create_swapp_note_with_pswap(
            charlie.id(),
            Asset::Fungible(FungibleAsset::new(usdc_faucet.id(), *charlie_usdc)?),
            Asset::Fungible(FungibleAsset::new(eth_faucet.id(), *charlie_eth)?),
            NoteType::Public,
            &mut note_rng,
        )?;
        builder.add_output_note(OutputNote::Full(charlie_swap_note.clone()));

        // Build tx_script from shared package
        let program = consume_asset_package.unwrap_program();
        let tx_script =
            TransactionScript::from_parts(program.mast_forest().clone(), program.entrypoint());

        let mock_chain = builder.build()?;

        // Note args: [unused, unused, inflight_amount, input_amount] in Word layout
        // On-chain: arg[0]=input_amount (last element), arg[1]=inflight_amount (second-to-last)
        let alice_note_args = Word::from([
            Felt::ZERO,
            Felt::ZERO,
            Felt::new(alice_fill_usdc),
            Felt::ZERO,
        ]);
        let charlie_note_args = Word::from([
            Felt::ZERO,
            Felt::ZERO,
            Felt::new(*charlie_fill_eth),
            Felt::ZERO,
        ]);
        let mut note_args_map = BTreeMap::new();
        note_args_map.insert(alice_swap_note.id(), alice_note_args);
        note_args_map.insert(charlie_swap_note.id(), charlie_note_args);

        // Pre-compute expected P2ID + Remainder notes using PswapNote::create_output_notes()
        let (alice_p2id_note, alice_remainder_opt) =
            PswapNote::create_output_notes(&alice_swap_note, bob.id(), 0, alice_fill_usdc)?;
        let alice_remainder_note = alice_remainder_opt.expect(&format!(
            "Case {}: Alice partial fill must produce remainder",
            i + 1
        ));

        let (charlie_p2id_note, charlie_remainder_opt) =
            PswapNote::create_output_notes(&charlie_swap_note, bob.id(), 0, *charlie_fill_eth)?;
        let charlie_remainder_note = charlie_remainder_opt.expect(&format!(
            "Case {}: Charlie partial fill must produce remainder",
            i + 1
        ));

        // Advice stack for spread (assets consumed directly into Bob's vault)
        let bob_asset_word = Word::from(Asset::from(FungibleAsset::new(eth_faucet.id(), spread)?));
        let advice_stack: Vec<Felt> = vec![
            bob_asset_word[0],
            bob_asset_word[1],
            bob_asset_word[2],
            bob_asset_word[3],
        ];

        let commitment_key: Word = Rpo256::hash_elements(&advice_stack);
        let mut commitment = commitment_key;
        commitment.reverse();

        // Save expected note IDs and assets before notes are moved
        let expected_note_checks = vec![
            (
                alice_p2id_note.id(),
                usdc_faucet.id(),
                alice_fill_usdc,
                "Alice P2ID",
            ),
            (
                alice_remainder_note.id(),
                eth_faucet.id(),
                alice_remaining_eth,
                "Alice remainder",
            ),
            (
                charlie_p2id_note.id(),
                eth_faucet.id(),
                *charlie_fill_eth,
                "Charlie P2ID",
            ),
            (
                charlie_remainder_note.id(),
                usdc_faucet.id(),
                charlie_remaining_usdc,
                "Charlie remainder",
            ),
        ];

        // Execute transaction: expect 4 output notes
        let tx_context = mock_chain
            .build_tx_context(
                bob.id(),
                &[alice_swap_note.id(), charlie_swap_note.id()],
                &[],
            )?
            .tx_script(tx_script)
            .tx_script_args(commitment)
            .extend_advice_map([(commitment_key, advice_stack)])
            .extend_expected_output_notes(vec![
                OutputNote::Full(alice_p2id_note),
                OutputNote::Full(alice_remainder_note),
                OutputNote::Full(charlie_p2id_note),
                OutputNote::Full(charlie_remainder_note),
            ])
            .extend_note_args(note_args_map)
            .build()?;

        let executed_tx = tx_context.execute().await?;

        // === Verify ===
        let output_notes = executed_tx.output_notes();
        assert_eq!(
            output_notes.num_notes(),
            4,
            "Case {}: Expected 4 notes (2 P2ID + 2 remainder), got {}",
            i + 1,
            output_notes.num_notes()
        );

        // Per-note asset verification: check each expected note's faucet_id and amount
        for (expected_id, expected_faucet, expected_amount, label) in &expected_note_checks {
            let mut found = false;
            for idx in 0..output_notes.num_notes() {
                let note = output_notes.get_note(idx);
                if note.id() == *expected_id {
                    let assets = note.assets().unwrap();
                    assert_eq!(
                        assets.num_assets(),
                        1,
                        "Case {}: {} should have exactly 1 asset",
                        i + 1,
                        label
                    );
                    if let Asset::Fungible(f) = assets.iter().next().unwrap() {
                        assert_eq!(
                            f.faucet_id(),
                            *expected_faucet,
                            "Case {}: {} faucet_id mismatch",
                            i + 1,
                            label
                        );
                        assert_eq!(
                            f.amount(),
                            *expected_amount,
                            "Case {}: {} amount mismatch (expected {}, got {})",
                            i + 1,
                            label,
                            expected_amount,
                            f.amount()
                        );
                    } else {
                        panic!("Case {}: {} asset is not fungible", i + 1, label);
                    }
                    found = true;
                    break;
                }
            }
            assert!(found, "Case {}: {} note not found in output", i + 1, label);
        }

        // Verify total asset conservation (notes only, spread goes to vault)
        let mut total_usdc_out: u64 = 0;
        let mut total_eth_out: u64 = 0;
        for (_, expected_faucet, expected_amount, _) in &expected_note_checks {
            if *expected_faucet == usdc_faucet.id() {
                total_usdc_out += expected_amount;
            } else if *expected_faucet == eth_faucet.id() {
                total_eth_out += expected_amount;
            }
        }
        // ETH conservation: notes output + spread in vault = total ETH input
        total_eth_out += spread;
        assert_eq!(
            total_usdc_out,
            *charlie_usdc,
            "Case {}: USDC conservation failed",
            i + 1
        );
        assert_eq!(
            total_eth_out,
            *alice_eth,
            "Case {}: ETH conservation failed",
            i + 1
        );

        // Bob's vault should have the spread added directly
        let vault_delta = executed_tx.account_delta().vault();
        assert_eq!(
            vault_delta.removed_assets().count(),
            0,
            "Case {}: Bob should not spend any assets",
            i + 1
        );
        let added_assets: Vec<Asset> = vault_delta.added_assets().collect();
        assert_eq!(
            added_assets.len(),
            1,
            "Case {}: Bob should receive 1 asset ({} ETH spread)",
            i + 1,
            spread
        );
        if let Asset::Fungible(f) = &added_assets[0] {
            assert_eq!(f.faucet_id(), eth_faucet.id(), "Case {}: Added asset should be ETH", i + 1);
            assert_eq!(f.amount(), spread, "Case {}: Bob should receive {} ETH spread", i + 1, spread);
        } else {
            panic!("Case {}: Expected fungible asset in Bob's vault", i + 1);
        }

        println!(
            "  PASSED: 4 notes | Alice P2ID {} USDC + Rem {}ETH | Charlie P2ID {} ETH + Rem {} USDC | Bob {} ETH spread (in vault)",
            alice_fill_usdc, alice_remaining_eth, charlie_fill_eth, charlie_remaining_usdc, spread
        );
    }

    println!(
        "\n=== All {} partial fill fuzz test cases passed! ===",
        total
    );
    Ok(())
}

/// Test proving that Felt field division and u64 integer division diverge
/// for non-exact ratios, causing the on-chain contract to produce different
/// results than the off-chain u64 `calculate_output_amount`.
///
/// Uses offered=100 USDC, requested=30 ETH (ratio 10/3 = 3.333...).
/// Bob partially fills with 7 ETH.
///
/// u64:  ratio = 100*100_000/30 = 333_333 (truncated), output = 7*333_333/100_000 = 23
/// Felt: ratio = 10_000_000 * inv(30) mod P (huge field element), output = garbage
///
/// The on-chain Felt result is a huge field element (~10^18), NOT 23.
/// This causes the transaction to fail because the asset amount is nonsensical.
#[tokio::test]
async fn swapp_note_non_exact_ratio_partial_fill_divergence_test() -> anyhow::Result<()> {
    println!("=== Test: Non-Exact Ratio Partial Fill (Felt vs u64 divergence) ===\n");

    // u64 calculate_output_amount for reference
    fn calculate_output_amount_u64(
        offered_total: u64,
        requested_total: u64,
        input_amount: u64,
    ) -> u64 {
        let precision_factor = 100_000u64;
        if offered_total > requested_total {
            let ratio = (offered_total * precision_factor) / requested_total;
            (input_amount * ratio) / precision_factor
        } else {
            let ratio = (requested_total * precision_factor) / offered_total;
            (input_amount * precision_factor) / ratio
        }
    }

    // Non-exact ratio: 100 USDC offered for 30 ETH (ratio = 10/3 = 3.333...)
    let offered_total = 100u64;
    let requested_total = 30u64;
    let input_amount = 7u64; // Partial fill: 7 of 30 ETH

    let u64_output = calculate_output_amount_u64(offered_total, requested_total, input_amount);
    println!(
        "Off-chain u64 prediction: offered_out = {} USDC",
        u64_output
    );
    println!(
        "  ratio = {} * 100_000 / {} = {} (truncated)",
        offered_total,
        requested_total,
        (offered_total * 100_000) / requested_total
    );
    println!(
        "  output = {} * {} / 100_000 = {} (truncated)",
        input_amount,
        (offered_total * 100_000) / requested_total,
        u64_output
    );

    // Show the Felt result for comparison
    use miden_core::FieldElement;
    let felt_offered = Felt::new(offered_total);
    let felt_requested = Felt::new(requested_total);
    let felt_input = Felt::new(input_amount);
    let felt_factor = Felt::new(100_000);

    // Mimic the on-chain logic
    let felt_ratio = (felt_offered * felt_factor) / felt_requested;
    let felt_output = (felt_input * felt_ratio) / felt_factor;
    println!(
        "\nOn-chain Felt prediction: offered_out = {} (as u64)",
        felt_output.as_int()
    );
    println!("  Felt ratio = {}", felt_ratio.as_int());
    println!("  These differ because Felt '/' is modular inverse, not integer truncation");
    println!(
        "\n  u64 says: {} USDC | Felt says: {} (huge field element)\n",
        u64_output,
        felt_output.as_int()
    );

    // Set up the actual on-chain test
    let mut builder = MockChain::builder();

    let usdc_faucet =
        builder.add_existing_basic_faucet(Auth::BasicAuth, "USDC", 10000, Some(1000))?;

    let eth_faucet = builder.add_existing_basic_faucet(Auth::BasicAuth, "ETH", 10000, Some(100))?;

    // Alice has 100 USDC, offers them for 30 ETH
    let alice = builder.add_existing_wallet_with_assets(
        Auth::BasicAuth,
        [FungibleAsset::new(usdc_faucet.id(), offered_total)?.into()],
    )?;
    println!("Alice: {:?} (has {} USDC)", alice.id(), offered_total);

    // Bob has 7 ETH, will partially fill
    let account_package = Arc::new(build_project_in_dir(
        Path::new("../contracts/basic-wallet"),
        true,
    )?);
    let bob = create_testing_account_from_package(
        account_package.clone(),
        AccountCreationConfig {
            storage_slots: vec![],
            ..Default::default()
        },
        vec![FungibleAsset::new(eth_faucet.id(), input_amount)?.into()],
    )
    .await?;
    println!("Bob:   {:?} (has {} ETH)", bob.id(), input_amount);

    builder.add_account(bob.clone())?;

    // Create swap note: Alice offers 100 USDC for 30 ETH
    let swapp_package = Arc::new(build_project_in_dir(
        Path::new("../contracts/swapp-note"),
        true,
    )?);

    let p2id_tag_felt = compute_p2id_tag_felt(alice.id());

    let note_inputs = vec![
        eth_faucet.id().prefix().into(),
        eth_faucet.id().suffix(),
        Felt::ZERO,
        Felt::new(requested_total), // requested 30 ETH
        alice.id().prefix().into(),
        alice.id().suffix(),
        NoteType::Public.into(),
        p2id_tag_felt,
    ];

    let offered_asset = FungibleAsset::new(usdc_faucet.id(), offered_total)?;
    let mut note_assets = NoteAssets::default();
    note_assets.add_asset(offered_asset.into())?;

    let swap_note = create_testing_note_from_package(
        swapp_package.clone(),
        alice.id(),
        NoteCreationConfig {
            assets: note_assets,
            inputs: note_inputs,
            ..Default::default()
        },
    )?;

    builder.add_output_note(OutputNote::Full(swap_note.clone()));

    let mock_chain = builder.build()?;

    // Bob provides 7 ETH (partial fill with input_amount, no inflight)
    let note_args = Word::from([Felt::ZERO, Felt::ZERO, Felt::ZERO, Felt::new(input_amount)]);
    let mut note_args_map = BTreeMap::new();
    note_args_map.insert(swap_note.id(), note_args);

    let tx_context = mock_chain
        .build_tx_context(bob.id(), &[swap_note.id()], &[])?
        .extend_note_args(note_args_map)
        .build()?;

    // Execute: on-chain Felt division will produce a huge field element as
    // the offered output, which should cause the transaction to fail
    println!(
        "Executing transaction (Bob provides {} ETH partial fill)...",
        input_amount
    );
    let result = tx_context.execute().await;

    if let Err(e) = &result {
        println!("\nTransaction FAILED as expected:");
        println!("  Error: {}", e);
        println!("\n  Root cause: Felt '/' computes modular inverse, not integer truncation.");
        println!(
            "  On-chain: calculate_output_amount(100, 30, 7) = {} (field element)",
            felt_output.as_int()
        );
        println!(
            "  Off-chain: calculate_output_amount(100, 30, 7) = {} (truncated)",
            u64_output
        );
        println!("  The Felt result is used as an asset amount, causing failure.");
    } else {
        let executed_tx = result.as_ref().unwrap();
        // If it somehow succeeds, check what Bob actually received
        let vault_delta = executed_tx.account_delta().vault();
        let added: Vec<Asset> = vault_delta.added_assets().collect();
        println!("\nTransaction SUCCEEDED (unexpected!)");
        println!("  Bob's added assets: {:?}", added);
        for asset in &added {
            if let Asset::Fungible(f) = asset {
                println!(
                    "  Bob received: {} of faucet {:?}",
                    f.amount(),
                    f.faucet_id()
                );
            }
        }
        let output_notes = executed_tx.output_notes();
        println!("  Output notes: {}", output_notes.num_notes());

        panic!(
            "Transaction succeeded but should have failed! \
             On-chain Felt division produces {} (not u64's {}). \
             This means the on-chain result is a meaningless field element.",
            felt_output.as_int(),
            u64_output
        );
    }

    println!("\n=== Non-exact ratio divergence test complete ===");
    Ok(())
}
