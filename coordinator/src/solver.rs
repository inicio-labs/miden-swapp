use anyhow::{Context, Result};
use miden_client::{
    keystore::FilesystemKeyStore,
    note::{Note, NoteAssets as ClientNoteAssets, NoteExecutionHint, NoteMetadata, NoteTag},
    transaction::TransactionRequestBuilder,
    utils::{Deserializable, Serializable},
    Client, Felt, Word,
};
use miden_core::FieldElement;
use miden_lib::note::utils::build_p2id_recipient;
use miden_objects::{account::AccountId, asset::FungibleAsset, note::NoteDetails};
use rand::rngs::StdRng;
use tokio::time::Duration;

use crate::note_pool::NotePool;

/// Check if a swap is viable based on Bob's vault assets
fn is_swap_viable(note: &Note, vault_assets: &[miden_objects::asset::Asset]) -> bool {
    // Extract requested asset from note inputs
    // Note inputs structure: [requested_asset_word (4 felts), creator_id (4 felts)]
    let inputs = note.inputs();
    if inputs.num_values() < 8 {
        return false;
    }

    let values = inputs.values();

    // Requested asset: positions 0-3
    let requested_faucet_prefix = values[0];
    let requested_faucet_suffix = values[1];
    let requested_amount = values[3].as_int();

    // Construct the faucet AccountId from prefix/suffix using TryFrom<[Felt; 2]>
    let requested_faucet_id =
        match AccountId::try_from([requested_faucet_prefix, requested_faucet_suffix]) {
            Ok(id) => id,
            Err(_) => return false,
        };

    // Check if Bob has enough of the requested asset
    for asset in vault_assets {
        if let miden_objects::asset::Asset::Fungible(fa) = asset {
            if fa.faucet_id() == requested_faucet_id {
                // Check if Bob has enough balance
                return fa.amount() >= requested_amount;
            }
        }
    }

    false
}

/// Execute a swap by consuming the note and creating a P2ID note for Alice
async fn execute_swap(
    client: &mut Client<FilesystemKeyStore<StdRng>>,
    bob_id: AccountId,
    note: &Note,
) -> Result<Note> {
    // Sync state first
    client.sync_state().await?;

    // Extract note details
    let inputs = note.inputs();
    let values = inputs.values();

    // Parse requested asset and amount
    let requested_faucet_prefix = values[0];
    let requested_faucet_suffix = values[1];
    let requested_amount = values[3].as_int();

    // Parse creator (Alice) account ID
    let creator_prefix = values[4];
    let creator_suffix = values[5];

    // Reconstruct faucet and creator IDs using TryFrom<[Felt; 2]>
    let requested_faucet_id =
        AccountId::try_from([requested_faucet_prefix, requested_faucet_suffix])?;
    let alice_id = AccountId::try_from([creator_prefix, creator_suffix])?;

    // Full fill: solver provides exactly the requested amount
    let solver_amount = requested_amount;

    // Create note args: [0, 0, inflight_amount=0, solver_amount]
    let note_args = Word::from([Felt::ZERO, Felt::ZERO, Felt::ZERO, Felt::new(solver_amount)]);

    // Construct expected P2ID note
    let p2id_serial_num = Word::from([
        note.recipient().serial_num()[0] + Felt::new(1),
        note.recipient().serial_num()[1] + Felt::new(1),
        note.recipient().serial_num()[2] + Felt::new(1),
        note.recipient().serial_num()[3] + Felt::new(1),
    ]);

    let p2id_recipient = build_p2id_recipient(alice_id, p2id_serial_num)
        .context("Failed to build P2ID recipient")?;

    // Compute P2ID tag from Alice's account ID (same as in integration tests)
    let p2id_tag = values[7];
    let p2id_tag = NoteTag::from(p2id_tag.as_int() as u32);
    let p2id_aux = Felt::new(solver_amount);
    let p2id_execution_hint = NoteExecutionHint::none();

    // Use the same note type as the incoming note (public or private)
    let p2id_note_type = note.metadata().note_type();

    let p2id_asset = FungibleAsset::new(requested_faucet_id, solver_amount)?;
    let p2id_note_assets = ClientNoteAssets::new(vec![p2id_asset.into()])
        .context("Failed to create P2ID note assets")?;

    let p2id_note_metadata = NoteMetadata::new(
        bob_id,
        p2id_note_type,
        p2id_tag,
        p2id_execution_hint,
        p2id_aux,
    )
    .context("Failed to create P2ID note metadata")?;

    let p2id_note = Note::new(p2id_note_assets, p2id_note_metadata, p2id_recipient.clone());
    let p2id_note_details = NoteDetails::from(&p2id_note);
    let expected_future_notes = vec![(p2id_note_details, p2id_tag)];
    println!(
        "Expected future notes: {:?}",
        p2id_note.script().root().to_hex()
    );

    // Build and submit transaction
    // Using unauthenticated_input_notes since swap notes are public and can be consumed by anyone
    let consume_request = TransactionRequestBuilder::new()
        .unauthenticated_input_notes([(note.clone(), Some(note_args))])
        .expected_future_notes(expected_future_notes)
        .expected_output_recipients(vec![p2id_recipient])
        .build()
        .context("Failed to build swap transaction")?;

    // Note: For unauthenticated notes, we don't need to import the note separately
    // The note is provided directly in unauthenticated_input_notes

    let tx_id = client
        .submit_new_transaction(bob_id, consume_request)
        .await
        .inspect(|tx_id| println!("Swap transaction submitted: {:?}", tx_id))
        .inspect_err(|e| println!("Failed to submit swap transaction: {:?}", e))
        .context("Failed to submit swap transaction")?;

    println!("         TX ID: {:?}", tx_id);

    Ok(p2id_note)
}

/// Main Bob solver loop - runs continuously in the background
pub async fn run_solver_loop(
    mut client: Client<FilesystemKeyStore<StdRng>>,
    bob_id: AccountId,
    note_pool: NotePool,
) {
    let timeout_secs = 300; // 5 minutes timeout for claimed notes

    loop {
        // Handle errors gracefully and continue looping
        if let Err(e) = solver_iteration(&mut client, bob_id, &note_pool, timeout_secs).await {
            println!("❌ [Bob Solver] Error in solver iteration: {}", e);
        }

        println!("⏸️  [Bob Solver] Scan complete. Sleeping for 5 seconds...\n");
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// Single iteration of the solver loop
async fn solver_iteration(
    client: &mut Client<FilesystemKeyStore<StdRng>>,
    bob_id: AccountId,
    note_pool: &NotePool,
    timeout_secs: u64,
) -> Result<()> {
    println!("🔍 [Bob Solver] Scanning cycle started...");

    // Sync with network first
    client.sync_state().await?;
    println!("✓ [Bob Solver] Synced with network");

    // Reset any timed-out claims
    note_pool.reset_timed_out_claims(timeout_secs);

    // === SCAN 1: API-Submitted Notes ===
    println!("📋 [Bob Solver] Checking API-submitted notes in pool...");
    let candidates = note_pool.get_pending_notes();
    println!("   Found {} pending API notes", candidates.len());

    for (note_id, note_entry) in candidates {
        // Deserialize note
        let note = match Note::read_from_bytes(&note_entry.raw_note_bytes) {
            Ok(n) => n,
            Err(e) => {
                println!("   ❌ Failed to deserialize note {}: {}", note_id, e);
                note_pool.mark_failed(&note_id, format!("Deserialization failed: {}", e));
                continue;
            }
        };

        println!("   🔎 Examining note {}", note_id);

        // Check Bob's balance
        let account = client.get_account(bob_id).await?;
        let vault_assets: Vec<_> = account
            .as_ref()
            .map(|acc| acc.account().vault().assets().collect())
            .unwrap_or_default();

        println!("      Bob's vault: {} assets", vault_assets.len());

        // Check if swap is viable
        if !is_swap_viable(&note, &vault_assets) {
            println!("      ❌ Not viable - insufficient balance");
            continue;
        }
        println!("      ✓ Swap is viable");

        // Claim the note
        if !note_pool.try_claim(&note_id) {
            println!("      ⚠️  Already claimed by another process");
            continue;
        }
        println!("      🔒 Claimed note {}", note_id);

        // Execute swap
        println!("      ⚙️  Executing swap transaction...");
        match execute_swap(client, bob_id, &note).await {
            Ok(p2id_note) => {
                let bytes = p2id_note.to_bytes();
                note_pool.mark_solved(&note_id, bytes);
                println!("      ✅ Swap executed successfully!");
                println!("         P2ID note created: {:?}", p2id_note.id());
            }
            Err(e) => {
                note_pool.mark_failed(&note_id, e.to_string());
                println!("      ❌ Swap failed: {}", e);
            }
        }
    }

    // === SCAN 2: Network Consumable Notes ===
    println!("🌐 [Bob Solver] Checking network consumable notes...");
    let consumable_notes = client.get_consumable_notes(Some(bob_id)).await?;
    println!(
        "   Found {} consumable notes on network",
        consumable_notes.len()
    );

    if !consumable_notes.is_empty() {
        let note_ids: Vec<_> = consumable_notes.iter().map(|(note, _)| note.id()).collect();
        println!("   Note IDs: {:?}", note_ids);
        println!("   💰 Consuming all consumable notes...");

        let tx_request = TransactionRequestBuilder::new()
            .build_consume_notes(note_ids.clone())
            .context("Failed to build consume transaction")?;

        match client.submit_new_transaction(bob_id, tx_request).await {
            Ok(tx_id) => {
                println!("   ✅ Consumed {} notes. TX: {:?}", note_ids.len(), tx_id);
            }
            Err(e) => {
                println!("   ❌ Failed to consume notes: {}", e);
            }
        }
    }

    Ok(())
}
