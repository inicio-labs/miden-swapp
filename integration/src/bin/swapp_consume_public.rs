use integration::helpers::{setup_client, ClientSetup};
use integration::swapp_state::SwappTestState;

use anyhow::{Context, Result};
use miden_client::{
    note::NoteType,
    transaction::{OutputNote, TransactionRequestBuilder},
    Felt, Word,
};
use miden_core::FieldElement;
use miden_protocol::{
    asset::{Asset, FungibleAsset},
    note::{NoteAttachment, NoteDetails},
};
use miden_swapp::PswapNote;
use tokio::time::Duration;

/// Swap Note Consumption Test Binary (using PSWAP.masm via PswapNote)
///
/// This binary tests swap note consumption after setup has been completed.
/// It loads the persisted state (account IDs, faucet IDs) and:
/// - Alice creates a swap note (Offer 50 USDT for 20 ETH)
/// - Bob fully fills the swap note with 20 ETH
///

#[tokio::main]
async fn main() -> Result<()> {
    println!("=== Swap Note Consumption Test (PswapNote) ===\n");

    //------------------------------------------------------------
    // Load persisted state
    //------------------------------------------------------------
    let state = SwappTestState::load()?;
    let faucet1_id = state.faucet1_id()?; // USDT
    let faucet2_id = state.faucet2_id()?; // ETH
    let alice_id = state.alice_id()?;
    let bob_id = state.bob_id()?;

    println!("USDT Faucet: {:?}", faucet1_id);
    println!("ETH Faucet: {:?}", faucet2_id);
    println!("Alice: {:?}", alice_id);
    println!("Bob: {:?}\n", bob_id);

    //------------------------------------------------------------
    // Initialize client
    //------------------------------------------------------------
    let ClientSetup { mut client, .. } = setup_client().await?;
    let sync_summary = client.sync_state().await?;
    println!("Latest block: {}\n", sync_summary.block_num);

    //------------------------------------------------------------
    // Alice creates swap note: Offer 50 USDT for 20 ETH
    //------------------------------------------------------------
    let offered_amount = 50u64;
    let requested_amount = 20u64;

    println!(
        "[1] Alice creates swap note ({} USDT -> {} ETH)",
        offered_amount, requested_amount
    );

    let swap_note = PswapNote::create(
        alice_id,
        Asset::Fungible(FungibleAsset::new(faucet1_id, offered_amount)?),
        Asset::Fungible(FungibleAsset::new(faucet2_id, requested_amount)?),
        NoteType::Public,
        NoteAttachment::default(),
        client.rng(),
    )
    .map_err(|e| anyhow::anyhow!("Failed to create swap note: {:?}", e))?;

    println!("Swap note created: {:?}", swap_note.id());

    let tx_id = client
        .submit_new_transaction(
            alice_id,
            TransactionRequestBuilder::new()
                .own_output_notes(vec![OutputNote::Full(swap_note.clone())])
                .build()
                .unwrap(),
        )
        .await?;

    println!("Published. TX: {:?}\n", tx_id);

    // Wait for note to be available
    println!("Waiting for swap note to be available...");
    tokio::time::sleep(Duration::from_secs(15)).await;
    client.sync_state().await?;
    client.sync_state().await?;

    //------------------------------------------------------------
    // Bob fully fills with 20 ETH
    //------------------------------------------------------------
    let full_fill_amount = requested_amount;
    println!(
        "[2] Bob fully fills with {} ETH (expects {} USDT)",
        full_fill_amount, offered_amount
    );

    // Note args: Word[3] = input_amount, Word[2] = inflight_amount
    let note_args = Word::from([
        Felt::ZERO,
        Felt::ZERO,
        Felt::ZERO,                  // inflight = 0
        Felt::new(full_fill_amount), // input = full fill
    ]);

    // Build expected P2ID output note using PswapNote helper
    let (p2id_note, _remainder) =
        PswapNote::create_output_notes(&swap_note, bob_id, full_fill_amount, 0)
            .map_err(|e| anyhow::anyhow!("Failed to create expected output notes: {:?}", e))?;

    println!("Expected P2ID note: {:?}", p2id_note.id());

    let _expected_future_notes = vec![(NoteDetails::from(&p2id_note), p2id_note.metadata().tag())];

    // Debug: print recipient details so we can compare
    println!(
        "Expected P2ID recipient digest: {:?}",
        p2id_note.recipient().digest().to_hex()
    );
    println!(
        "Expected P2ID serial: {:?}",
        p2id_note.recipient().serial_num()
    );

    let consume_request = TransactionRequestBuilder::new()
        .input_notes(vec![(swap_note.clone(), Some(note_args))])
        //.expected_future_notes(expected_future_notes)
        .build()
        .context("Failed to build consume transaction")?;

    let tx_id = client
        .submit_new_transaction(bob_id, consume_request)
        .await
        .context("Failed to execute swap note consumption")?;
    println!("Bob consumed swap note. TX: {:?}", tx_id);
    println!(
        "Expected: Bob receives {} USDT, Alice receives {} ETH via P2ID\n",
        offered_amount, full_fill_amount
    );

    // Wait for Bob's TX to be included in a block, then sync
    println!("Waiting for block confirmation...");
    tokio::time::sleep(Duration::from_secs(30)).await;
    client.sync_state().await?;
    tokio::time::sleep(Duration::from_secs(10)).await;
    client.sync_state().await?;

    //------------------------------------------------------------
    // Alice finds and consumes the P2ID note from chain
    //------------------------------------------------------------
    println!("\n[3] Alice looking for consumable P2ID note...");

    let consumable = client.get_consumable_notes(Some(alice_id)).await?;
    println!("Found {} consumable notes for Alice", consumable.len());

    for (note, relevance) in &consumable {
        println!("  Note ID: {:?}", note.id());
        println!("  Assets: {:?}", note.assets());
        println!("  Relevance: {:?}", relevance);
    }

    if let Some((target_note, _)) = consumable.first() {
        println!("\nConsuming note: {:?}", target_note.id());

        let consume_tx = TransactionRequestBuilder::new()
            .input_notes(vec![(target_note.clone().try_into()?, None)])
            .build()
            .context("Failed to build Alice's consume transaction")?;

        match client.submit_new_transaction(alice_id, consume_tx).await {
            Ok(tx_id) => println!("SUCCESS! Alice consumed P2ID note. TX: {:?}", tx_id),
            Err(e) => println!("FAILED: {:?}", e),
        }
    } else {
        println!("No consumable notes found for Alice yet.");
        println!("Try syncing again after a few blocks.");
    }

    println!("\n=== Test Complete ===");
    Ok(())
}
