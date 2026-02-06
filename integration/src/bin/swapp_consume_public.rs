use integration::helpers::{
    build_project_in_dir, compute_p2id_tag_felt, compute_p2id_tag_for_local_account,
    create_testing_note_from_package, setup_client, ClientSetup, NoteCreationConfig,
};
use integration::swapp_state::SwappTestState;

use anyhow::{Context, Result};
use miden_client::{
    note::{Note, NoteAssets as ClientNoteAssets, NoteExecutionHint, NoteMetadata, NoteType},
    transaction::{OutputNote, TransactionRequestBuilder},
    Felt, Word,
};
use miden_core::FieldElement;
use miden_lib::note::utils::build_p2id_recipient;
use miden_objects::{
    asset::FungibleAsset,
    note::{NoteAssets, NoteDetails},
};
use std::{path::Path, sync::Arc};
use tokio::time::Duration;

/// Swap Note Consumption Test Binary
///
/// This binary tests swap note consumption after setup has been completed.
/// It loads the persisted state (account IDs, faucet IDs) and:
/// - Alice creates a swap note (Offer 50 USDT for 25 ETH)
/// - Bob fully fills the swap note with 25 ETH
///
/// Run this binary multiple times to iterate on swap note consumption logic
/// without redoing the expensive setup steps

#[tokio::main]
async fn main() -> Result<()> {
    println!("=== Swap Note Consumption Test ===\n");

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
    // SCENARIO: Alice creates swap note, Bob fully fills
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
    // - Positions 0-3: Requested Asset Word (Faucet2.prefix, Faucet2.suffix, 0, 75)
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

    //bob balances before full fill
    let bob_account = client
        .get_account(bob_id)
        .await?
        .context("Failed to get bob account")?;

    let bob_vault = bob_account.account().vault().assets().collect::<Vec<_>>();

    println!("Bob vault before full fill: {:?}", bob_vault);

    //------------------------------------------------------------
    // Bob fully fills the swap note with 25 ETH
    //------------------------------------------------------------
    println!("Bob fully fills with 25 ETH (expects 50 USDT)");

    let full_fill_amount = 25u64;

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

    println!("Bob consuming the swap note directly (unauthenticated)");
    println!("Note: Bob needs 25 ETH in his vault to fulfill this swap");

    // Sync state again right before building the transaction to ensure latest state
    client.sync_state().await?;

    // Create note args: [0, 0, inflight_amount, solver_amount]
    // For regular swap: inflight_amount = 0, solver_amount = input_amount
    let note_args = Word::from([
        Felt::ZERO,
        Felt::ZERO,
        Felt::ZERO,                  // inflight_amount = 0 (not an inflight swap)
        Felt::new(full_fill_amount), // solver_amount = input_amount (25 ETH)
    ]);

    // Construct expected P2ID note that will be created by the swap script
    // This note will contain 25 ETH and be sent to Alice
    println!("\nCreating expected P2ID note for Alice (25 ETH)...");

    let p2id_serial_num = Word::from([
        published_swap_note.recipient().serial_num()[0] + Felt::new(1),
        published_swap_note.recipient().serial_num()[1] + Felt::new(1),
        published_swap_note.recipient().serial_num()[2] + Felt::new(1),
        published_swap_note.recipient().serial_num()[3] + Felt::new(1),
    ]);

    let p2id_recipient = build_p2id_recipient(alice_id, p2id_serial_num)
        .context("Failed to build P2ID recipient")?;

    let p2id_tag = compute_p2id_tag_for_local_account(alice_id);
    let p2id_aux = Felt::new(full_fill_amount);
    let p2id_execution_hint = NoteExecutionHint::none();

    let p2id_asset = FungibleAsset::new(faucet2_id, full_fill_amount)?;
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

    // Construct the P2ID note and convert to NoteDetails for expected_future_notes
    // This is required so the advice provider has the note details when the script creates it
    let p2id_note = Note::new(p2id_note_assets, p2id_note_metadata, p2id_recipient.clone());
    let p2id_note_details = NoteDetails::from(&p2id_note);
    let expected_future_notes = vec![(p2id_note_details.clone(), p2id_tag)];

    // Print all P2ID note details for debugging
    println!("\n=== P2ID Note Details ===");
    println!("P2ID Note ID: {:?}", p2id_note.id());
    println!("P2ID Note Details (NoteDetails): {:?}", p2id_note_details);

    // Print metadata
    println!("\n--- Metadata ---");
    println!("Sender: {:?}", p2id_note.metadata().sender());
    println!("Note Type: {:?}", p2id_note.metadata().note_type());
    println!("Tag: {:?}", p2id_note.metadata().tag());
    println!("Aux: {:?}", p2id_note.metadata().aux());
    println!(
        "Execution Hint: {:?}",
        p2id_note.metadata().execution_hint()
    );

    // Print recipient
    println!("\n--- Recipient ---");
    println!(
        "Recipient Serial Num: {:?}",
        p2id_note.recipient().serial_num()
    );
    let recipient_digest = p2id_note.recipient().digest();
    println!("Recipient Digest: {:?}", recipient_digest.to_hex());
    println!("Recipient Script: {:?}", p2id_note.recipient().script());
    println!("Recipient Inputs: {:?}", p2id_note.recipient().inputs());

    // Print assets
    println!("\n--- Assets ---");
    let assets = p2id_note.assets();
    println!("Number of assets: {}", assets.num_assets());
    for (idx, asset) in assets.iter().enumerate() {
        println!("Asset {}: {:?}", idx, asset);
        if let miden_objects::asset::Asset::Fungible(fa) = asset {
            println!("  Faucet ID: {:?}", fa.faucet_id());
            println!("  Amount: {}", fa.amount());
        }
    }
    // Print NoteDetails structure

    // Build consume transaction using authenticated_input_notes with note args
    // Bob is consuming the swap note and providing the input amount
    // The swap note is authenticated, so we use authenticated_input_notes
    // authenticated_input_notes expects (NoteId, Option<Word>), not (Note, Option<Word>)
    // We pass Some(note_args) to specify the input_amount
    // For full fill, only 1 P2ID note is created (no remainder note)
    // The P2ID note will be created dynamically by the swap script, so we don't need to specify it
    // The client will automatically track notes created by the script
    let consume_request = TransactionRequestBuilder::new()
        .authenticated_input_notes([(published_swap_note.id(), Some(note_args))])
        .expected_future_notes(expected_future_notes)
        .expected_output_recipients(vec![p2id_recipient.clone()])
        .build()
        .context("Failed to build consume note transaction request")?;

    let tx_id = client
        .submit_new_transaction(bob_id, consume_request)
        .await
        .context("Failed to execute swap note consumption transaction")?;
    println!("Bob consumed swap note (full fill). TX: {:?}", tx_id);

    // Expected: Bob receives 50 USDT, Alice receives 25 ETH via P2ID
    // No remainder note created (full fill)
    println!("Expected: Bob receives 50 USDT, Alice receives 25 ETH");
    println!("No remainder note created (swap fully completed)");

    // IMPORTANT: Wait for transaction to be included in a block before proceeding
    // This ensures the transaction is fully processed and the database state is consistent
    // Wait a bit for transaction to be submitted
    tokio::time::sleep(Duration::from_secs(60)).await;
    client.sync_state().await?;
    println!("Full fill completed\n");

    println!("\n=== Test Complete ===");

    Ok(())
}
