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

/// Private Swap Note Consumption Test (using PSWAP.masm via PswapNote)
///
/// - Alice creates a PRIVATE swap note (Offer 50 USDT for 20 ETH)
/// - Bob fully fills with 20 ETH
/// - Output P2ID note inherits Private type from swap note

#[tokio::main]
async fn main() -> Result<()> {
    println!("=== Private Swap Note Consumption Test (PswapNote) ===\n");

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
    // Alice creates PRIVATE swap note: Offer 50 USDT for 20 ETH
    //------------------------------------------------------------
    let offered_amount = 50u64;
    let requested_amount = 20u64;

    println!(
        "[1] Alice creates PRIVATE swap note ({} USDT -> {} ETH)",
        offered_amount, requested_amount
    );

    let swap_note = PswapNote::create(
        alice_id,
        Asset::Fungible(FungibleAsset::new(faucet1_id, offered_amount)?),
        Asset::Fungible(FungibleAsset::new(faucet2_id, requested_amount)?),
        NoteType::Private,
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
    println!("P2ID note type: {:?}", p2id_note.metadata().note_type());

    let _expected_future_notes =
        vec![(NoteDetails::from(&p2id_note), p2id_note.metadata().tag())];

    let consume_request = TransactionRequestBuilder::new()
        .input_notes(vec![(swap_note.clone(), Some(note_args))])
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
    // Alice directly consumes the private P2ID note
    //------------------------------------------------------------
    println!("\n[3] Alice consuming private P2ID note ({} ETH)", full_fill_amount);
    println!("P2ID note ID: {:?}", p2id_note.id());

    let alice_consume_tx = TransactionRequestBuilder::new()
        .input_notes(vec![(p2id_note.clone(), None)])
        .build()
        .context("Failed to build Alice's consume transaction")?;

    match client.submit_new_transaction(alice_id, alice_consume_tx).await {
        Ok(tx_id) => println!("SUCCESS! Alice consumed P2ID note. TX: {:?}", tx_id),
        Err(e) => println!("FAILED: {:?}", e),
    }

    println!("\n=== Test Complete ===");
    Ok(())
}
