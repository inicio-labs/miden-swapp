use integration::helpers::{
    build_project_in_dir, compute_p2id_tag_felt, compute_p2id_tag_for_local_account,
    create_testing_note_from_package, setup_client, ClientSetup, NoteCreationConfig,
};
use integration::swapp_state::SwappTestState;

use anyhow::{Context, Result};
use miden_client::{
    note::{
        Note, NoteAssets as ClientNoteAssets, NoteExecutionHint, NoteInputs, NoteMetadata,
        NoteScript, NoteType,
    },
    transaction::{OutputNote, TransactionRequestBuilder},
    Felt, Word,
};
use miden_core::FieldElement;
use miden_lib::note::utils::build_p2id_recipient;
use miden_objects::{
    asset::FungibleAsset,
    note::{NoteAssets, NoteDetails, NoteRecipient},
};
use std::{path::Path, sync::Arc};
use tokio::time::Duration;

/// Swap Note Partial Fill Test Binary
///
/// This binary tests partial swap note consumption after setup has been completed.
/// It loads the persisted state (account IDs, faucet IDs) and:
/// - Alice creates a swap note (Offer 50 USDT for 25 ETH)
/// - Bob partially fills the swap note with 15 ETH (60% of requested)
/// - Creates P2ID note with 15 ETH for Alice
/// - Creates remainder swap note with 20 USDT for 10 ETH
///
/// Run this binary multiple times to iterate on partial fill logic
/// without redoing the expensive setup steps

#[tokio::main]
async fn main() -> Result<()> {
    println!("=== Swap Note Partial Fill Test ===\n");

    //------------------------------------------------------------
    // Load persisted state
    //------------------------------------------------------------
    println!("[LOADING STATE]");
    let state = SwappTestState::load()?;
    let faucet1_id = state.faucet1_id()?;
    let faucet2_id = state.faucet2_id()?;
    let alice_id = state.alice_id()?;
    let bob_id = state.bob_id()?;

    println!("Faucet 1 (USDT): {:?}", faucet1_id);
    println!("Faucet 2 (ETH): {:?}", faucet2_id);
    println!("Alice: {:?}", alice_id);
    println!("Bob: {:?}\n", bob_id);

    //------------------------------------------------------------
    // Initialize client and build contract
    //------------------------------------------------------------
    println!("[SETUP] Initializing client and building contract");

    let ClientSetup { mut client, .. } = setup_client().await?;

    let sync_summary = client.sync_state().await?;
    println!("Latest block: {}\n", sync_summary.block_num);

    // Build the swapp-note contract
    let note_package = Arc::new(
        build_project_in_dir(Path::new("contracts/swapp-note"), true)
            .context("Failed to build swapp note contract")?,
    );
    println!("Swapp note contract built successfully\n");

    //------------------------------------------------------------
    // SCENARIO: Alice creates swap note, Bob partially fills
    //------------------------------------------------------------
    println!("[TEST] Alice creates swap note (Offer 50 USDT for 25 ETH)");

    // Alice creates a swap note: Offer 50 USDT for 25 ETH
    let offered_amount = 50u64;
    let requested_amount = 25u64;

    // Create the offered asset (50 USDT)
    let offered_asset = FungibleAsset::new(faucet1_id, offered_amount).unwrap();
    let mut note_assets = NoteAssets::default();
    note_assets.add_asset(offered_asset.into())?;

    // Compute P2ID tag for Alice (who will receive the output note)
    let p2id_tag_felt = compute_p2id_tag_felt(alice_id);

    // Build note inputs (8 Felts):
    // - Positions 0-3: Requested Asset Word (Faucet2.prefix, Faucet2.suffix, 0, 25)
    // - Positions 4-6: Creator AccountId (Alice.prefix, Alice.suffix, 0)
    // - Position 7: P2ID Tag
    let note_inputs = vec![
        // Requested Asset Word (ETH)
        faucet2_id.prefix().into(),
        faucet2_id.suffix().into(),
        Felt::ZERO,
        Felt::new(requested_amount),
        // Creator AccountId (Alice)
        alice_id.prefix().into(),
        alice_id.suffix().into(),
        Felt::ZERO,
        // P2ID Tag (position 7): computed tag for Alice
        p2id_tag_felt,
    ];

    // Create the swap note using the helper function
    let swap_note = create_testing_note_from_package(
        note_package.clone(),
        alice_id,
        NoteCreationConfig {
            assets: note_assets,
            inputs: note_inputs,
            ..Default::default()
        },
    )
    .context("Failed to create swap note")?;

    println!("Swap note created: {:?}", swap_note.id());

    // Publish the swap note
    let note_publish_request = TransactionRequestBuilder::new()
        .own_output_notes(vec![OutputNote::Full(swap_note.clone())])
        .build()
        .unwrap();

    let tx_id = client
        .submit_new_transaction(alice_id, note_publish_request)
        .await?;
    println!("Swap note published. TX: {:?}", tx_id);

    // Wait for note to be available
    tokio::time::sleep(Duration::from_secs(5)).await;
    client.sync_state().await?;
    println!("Swap note is now available\n");

    // Bob balances before partial fill
    let bob_account = client
        .get_account(bob_id)
        .await?
        .context("Failed to get bob account")?;

    let bob_vault = bob_account.account().vault().assets().collect::<Vec<_>>();

    println!("Bob vault before partial fill: {:?}", bob_vault);

    //------------------------------------------------------------
    // Bob partially fills the swap note with 15 ETH
    //------------------------------------------------------------
    println!("Bob partially fills with 15 ETH (expects 30 USDT, 60% of 50)");

    let partial_fill_amount = 15u64;

    // Wait for the swap note to be published and available
    println!("Waiting for swap note to be available...");
    tokio::time::sleep(Duration::from_secs(10)).await;

    // CRITICAL: Sync state to get the latest account state before building transaction
    // This ensures Bob's account state commitment matches what's on-chain
    println!("Syncing client state to get latest account state...");
    client.sync_state().await?;

    // Get the published swap note from the client's store
    // We need the actual Note object from the store, not the local one
    let input_notes = client
        .get_input_notes(miden_client::store::NoteFilter::All)
        .await?;

    let published_swap_note: Note = input_notes
        .iter()
        .find(|note_record| note_record.id() == swap_note.id())
        .ok_or_else(|| anyhow::anyhow!("Swap note not found in client store"))?
        .clone()
        .try_into()
        .context("Failed to convert note record to note")?;

    println!("Bob consuming the swap note directly (authenticated)");
    println!("Note: Bob needs 15 ETH in his vault to fulfill this partial swap");

    // Sync state again right before building the transaction to ensure latest state
    client.sync_state().await?;

    // Create note args: [0, 0, inflight_amount, solver_amount]
    // For regular swap: inflight_amount = 0, solver_amount = input_amount
    let note_args = Word::from([
        Felt::ZERO,
        Felt::ZERO,
        Felt::ZERO,                     // inflight_amount = 0 (not an inflight swap)
        Felt::new(partial_fill_amount), // solver_amount = input_amount (15 ETH)
    ]);

    // Calculate expected amounts for partial fill
    // offered_out = (input_amount * offered_total) / requested_total
    // offered_out = (15 * 50) / 25 = 30 USDT
    // remainder_offered = 50 - 30 = 20 USDT
    // remainder_requested = 25 - 15 = 10 ETH

    // Construct expected P2ID note that will be created by the swap script
    // This note will contain 15 ETH (input_amount) and be sent to Alice
    println!("\nCreating expected P2ID note for Alice (15 ETH)...");

    let p2id_serial_num = Word::from([
        published_swap_note.recipient().serial_num()[0] + Felt::new(1),
        published_swap_note.recipient().serial_num()[1] + Felt::new(1),
        published_swap_note.recipient().serial_num()[2] + Felt::new(1),
        published_swap_note.recipient().serial_num()[3] + Felt::new(1),
    ]);

    let p2id_recipient = build_p2id_recipient(alice_id, p2id_serial_num)
        .context("Failed to build P2ID recipient")?;

    let p2id_tag = compute_p2id_tag_for_local_account(alice_id);
    let p2id_aux = Felt::new(partial_fill_amount); // input_amount = 15
    let p2id_execution_hint = NoteExecutionHint::none();

    let p2id_asset = FungibleAsset::new(faucet2_id, partial_fill_amount)?; // 15 ETH
    let p2id_note_assets = ClientNoteAssets::new(vec![p2id_asset.into()])
        .context("Failed to create P2ID note assets")?;

    let p2id_note_metadata = NoteMetadata::new(
        bob_id,
        NoteType::Public,
        p2id_tag,
        p2id_execution_hint,
        p2id_aux,
    )
    .context("Failed to create P2ID note metadata")?;

    let p2id_note = Note::new(p2id_note_assets, p2id_note_metadata, p2id_recipient.clone());
    let p2id_note_details = NoteDetails::from(&p2id_note);

    // Create the expected remainder swap note (contains 20 USDT, requests 10 ETH)
    println!("\nCreating expected remainder swap note (20 USDT for 10 ETH)...");

    let current_note_serial = published_swap_note.recipient().serial_num();
    let serial_num_array: [Felt; 4] = current_note_serial.into();
    // Serial number is RPO hash of the existing swap note's serial number
    let remainder_serial_num: [Felt; 4] =
        miden_core::crypto::hash::Rpo256::hash_elements(&serial_num_array).into();
    let remainder_serial_num = Word::from(remainder_serial_num);
    println!("Remainder serial num: {:?}", remainder_serial_num);

    // Note inputs for remainder swap note: requesting 10 ETH, creator is Alice
    let remainder_note_inputs = vec![
        // Requested Asset (positions 0-3): 10 ETH (remaining)
        faucet2_id.prefix().into(),
        faucet2_id.suffix().into(),
        Felt::ZERO,
        Felt::new(10), // requested_asset_total (25 - 15 = 10)
        // Note Creator (positions 4-6): Alice
        alice_id.prefix().into(),
        alice_id.suffix().into(),
        Felt::ZERO,
        // P2ID Tag (position 7): computed tag for Alice (reusing)
        p2id_tag_felt,
    ];

    // Create recipient for remainder swap note
    let note_program = note_package.unwrap_program();
    let remainder_note_script = NoteScript::from_parts(
        note_program.mast_forest().clone(),
        note_program.entrypoint(),
    );
    let remainder_note_inputs_obj =
        NoteInputs::new(remainder_note_inputs).context("Failed to create remainder note inputs")?;
    let remainder_recipient = NoteRecipient::new(
        remainder_serial_num,
        remainder_note_script,
        remainder_note_inputs_obj,
    );

    // Create metadata for remainder note
    let remainder_tag = published_swap_note.metadata().tag();
    let remainder_aux = Felt::new(30); // offered_out = (50 * 15) / 25 = 30
    let remainder_note_metadata = NoteMetadata::new(
        bob_id,
        NoteType::Public,
        remainder_tag,
        NoteExecutionHint::none(),
        remainder_aux,
    )
    .context("Failed to create remainder note metadata")?;

    // Create assets for remainder note: 20 USDT (50 - 30 = 20)
    let remainder_asset = FungibleAsset::new(faucet1_id, 20)?;
    let remainder_note_assets = ClientNoteAssets::new(vec![remainder_asset.into()])
        .context("Failed to create remainder note assets")?;

    let remainder_note = Note::new(
        remainder_note_assets,
        remainder_note_metadata,
        remainder_recipient.clone(),
    );
    let remainder_note_details = NoteDetails::from(&remainder_note);

    // Expected future notes: both P2ID note and remainder note
    let expected_future_notes = vec![
        (p2id_note_details.clone(), p2id_tag),
        (remainder_note_details.clone(), remainder_tag),
    ];

    // Print note details for debugging
    println!("\n=== P2ID Note Details ===");
    println!("P2ID Note ID: {:?}", p2id_note.id());
    println!("P2ID Note contains: 15 ETH for Alice");

    println!("\n=== Remainder Note Details ===");
    println!("Remainder Note ID: {:?}", remainder_note.id());
    println!("Remainder Note contains: 20 USDT, requests 10 ETH");

    // Build consume transaction using authenticated_input_notes with note args
    // Bob is consuming the swap note and providing the input amount
    // The swap note is authenticated, so we use authenticated_input_notes
    // authenticated_input_notes expects (NoteId, Option<Word>), not (Note, Option<Word>)
    // We pass Some(note_args) to specify the input_amount
    // For partial fill, 2 notes are created: P2ID note + remainder swap note
    let consume_request = TransactionRequestBuilder::new()
        .authenticated_input_notes([(published_swap_note.id(), Some(note_args))])
        .expected_future_notes(expected_future_notes)
        .expected_output_recipients(vec![p2id_recipient.clone(), remainder_recipient.clone()])
        .build()
        .context("Failed to build consume note transaction request")?;

    let tx_id = client
        .submit_new_transaction(bob_id, consume_request)
        .await
        .context("Failed to execute swap note consumption transaction")?;
    println!("Bob consumed swap note (partial fill). TX: {:?}", tx_id);

    // Expected: Bob receives 30 USDT, Alice receives 15 ETH via P2ID
    // Remainder note created: 20 USDT for 10 ETH
    println!("Expected: Bob receives 30 USDT, Alice receives 15 ETH");
    println!("Remainder note created: 20 USDT for 10 ETH");

    // Wait for transaction to be submitted
    tokio::time::sleep(Duration::from_secs(5)).await;
    client.sync_state().await?;
    println!("Partial fill completed\n");

    println!("\n=== Test Complete ===");
    println!("Summary:");
    println!("  - Bob provided 15 ETH (60% of requested 25)");
    println!("  - Bob received 30 USDT (60% of offered 50)");
    println!("  - P2ID note created for Alice with 15 ETH");
    println!("  - Remainder swap note created: 20 USDT for 10 ETH");

    Ok(())
}
