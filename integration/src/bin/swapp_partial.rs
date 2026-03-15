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

/// Swap Note Partial Fill Test Binary (using PSWAP.masm via PswapNote)
///
/// - Alice creates a swap note (Offer 50 USDT for 25 ETH)
/// - Bob partially fills with 15 ETH (60% of requested)
/// - Creates P2ID note with 15 ETH for Alice
/// - Creates remainder swap note with 20 USDT for 10 ETH

#[tokio::main]
async fn main() -> Result<()> {
    println!("=== Swap Note Partial Fill Test (PswapNote) ===\n");

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
    // Alice creates swap note: Offer 50 USDT for 25 ETH
    //------------------------------------------------------------
    let offered_amount = 50u64;
    let requested_amount = 25u64;
    let partial_fill_amount = 15u64;

    println!("[1] Alice creates swap note ({} USDT -> {} ETH)", offered_amount, requested_amount);

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
    // Bob partially fills with 15 ETH
    //------------------------------------------------------------
    let offered_out = PswapNote::calculate_output_amount(offered_amount, requested_amount, partial_fill_amount);
    let remaining_offered = offered_amount - offered_out;
    let remaining_requested = requested_amount - partial_fill_amount;

    println!("[2] Bob partially fills with {} ETH (expects {} USDT)", partial_fill_amount, offered_out);
    println!("    Remainder: {} USDT for {} ETH", remaining_offered, remaining_requested);

    // Note args: Word[3] = input_amount, Word[2] = inflight_amount
    let note_args = Word::from([
        Felt::ZERO,
        Felt::ZERO,
        Felt::ZERO,                     // inflight = 0
        Felt::new(partial_fill_amount), // input = 15 ETH
    ]);

    // Build expected output notes using PswapNote helper
    let (p2id_note, remainder_note) =
        PswapNote::create_output_notes(&swap_note, bob_id, partial_fill_amount, 0)
            .map_err(|e| anyhow::anyhow!("Failed to create expected output notes: {:?}", e))?;

    println!("Expected P2ID note: {:?}", p2id_note.id());

    let mut expected_future_notes = vec![(
        NoteDetails::from(&p2id_note),
        p2id_note.metadata().tag(),
    )];

    if let Some(ref remainder) = remainder_note {
        println!("Expected remainder note: {:?}", remainder.id());
        expected_future_notes.push((
            NoteDetails::from(remainder),
            remainder.metadata().tag(),
        ));
    }

    let consume_request = TransactionRequestBuilder::new()
        .input_notes(vec![(swap_note.clone(), Some(note_args))])
        .expected_future_notes(expected_future_notes)
        .build()
        .context("Failed to build consume transaction")?;

    let tx_id = client
        .submit_new_transaction(bob_id, consume_request)
        .await
        .context("Failed to execute partial fill transaction")?;
    println!("Bob consumed swap note (partial fill). TX: {:?}", tx_id);

    println!("\nExpected results:");
    println!("  - Bob receives {} USDT ({}% of offered {})", offered_out, (partial_fill_amount * 100) / requested_amount, offered_amount);
    println!("  - P2ID note: {} ETH for Alice", partial_fill_amount);
    println!("  - Remainder note: {} USDT for {} ETH", remaining_offered, remaining_requested);

    // Wait for processing
    println!("\nWaiting for transaction to be processed...");
    tokio::time::sleep(Duration::from_secs(60)).await;
    client.sync_state().await?;

    //------------------------------------------------------------
    // Alice consumes the P2ID note
    //------------------------------------------------------------
    println!("\n[3] Alice consuming P2ID note ({} ETH)", partial_fill_amount);

    match client
        .submit_new_transaction(
            alice_id,
            TransactionRequestBuilder::new()
                .input_notes(vec![(p2id_note.clone(), None)])
                .build()?,
        )
        .await
    {
        Ok(tx_id) => println!("SUCCESS! Alice consumed P2ID note. TX: {:?}", tx_id),
        Err(e) => println!("FAILED: {:?}", e),
    }

    println!("\n=== Test Complete ===");
    Ok(())
}
