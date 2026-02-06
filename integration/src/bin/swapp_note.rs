use integration::helpers::{
    build_project_in_dir, compute_p2id_tag_felt, create_testing_note_from_package, setup_client,
    ClientSetup, NoteCreationConfig,
};

use anyhow::{Context, Result};
use miden_client::{
    account::component::{BasicFungibleFaucet, BasicWallet},
    auth::AuthSecretKey,
    note::NoteType,
    transaction::{OutputNote, TransactionRequestBuilder},
    Felt, Word,
};
use miden_core::FieldElement;
use miden_lib::account::auth::AuthRpoFalcon512;
use miden_objects::{
    account::{AccountBuilder, AccountStorageMode, AccountType},
    asset::{FungibleAsset, TokenSymbol},
    note::{NoteAssets, NoteDetails, NoteTag},
};
use rand::RngCore;
use std::{path::Path, sync::Arc};
use tokio::time::Duration;

/// Swap Note Integration Binary
///
/// This binary demonstrates end-to-end swap note functionality with:
/// - Two faucets (USDT and ETH)
/// - Two wallet accounts (Alice and Bob)
/// - Partial fill scenario: Bob provides 40 ETH for Alice's offer of 50 USDT for 75 ETH
/// - Full fill scenario: Bob provides the remaining 35 ETH
///
/// The swap note contract expects note args in the format: [input_amount, 0, 0, 0]
/// These note args are passed via the `Option<NoteArgs>` parameter in
/// `unauthenticated_input_notes([(note, Some(note_args))])`.
///
/// Expected behavior:
/// - Partial fill: 40 ETH → ~26.67 USDT (proportional to 40/75 of 50)
/// - Full fill: 35 ETH → ~23.33 USDT (remaining amount)
/// - Alice receives: 40 + 35 = 75 ETH via P2ID routing notes
/// - Bob receives: ~50 USDT total
///

#[tokio::main]
async fn main() -> Result<()> {
    println!("=== Swap Note Integration Test ===\n");

    //------------------------------------------------------------
    // STEP 1: Initialize client and build contract
    //------------------------------------------------------------
    println!("[STEP 1] Initializing client and building contract");

    let ClientSetup {
        mut client,
        keystore,
    } = setup_client().await?;

    let sync_summary = client.sync_state().await?;
    println!("Latest block: {}\n", sync_summary.block_num);

    // Build the swapp-note contract
    let note_package = Arc::new(
        build_project_in_dir(Path::new("../contracts/swapp-note"), true)
            .context("Failed to build swapp note contract")?,
    );
    println!("Swapp note contract built successfully\n");

    //------------------------------------------------------------
    // STEP 2: Deploy two fungible faucets (USDT and ETH)
    //------------------------------------------------------------
    println!("[STEP 2] Deploying two fungible faucets");

    // Faucet 1: USDT
    let mut init_seed = [0u8; 32];
    client.rng().fill_bytes(&mut init_seed);

    let symbol_usdt = TokenSymbol::new("USDT").unwrap();
    let decimals = 8;
    let max_supply = Felt::new(1_000_000);

    let key_pair_faucet1 = AuthSecretKey::new_rpo_falcon512();

    let faucet1_account = AccountBuilder::new(init_seed)
        .account_type(AccountType::FungibleFaucet)
        .storage_mode(AccountStorageMode::Public)
        .with_auth_component(AuthRpoFalcon512::new(
            key_pair_faucet1.public_key().to_commitment(),
        ))
        .with_component(BasicFungibleFaucet::new(symbol_usdt, decimals, max_supply).unwrap())
        .build()
        .unwrap();

    client.add_account(&faucet1_account, false).await?;
    keystore.add_key(&key_pair_faucet1).unwrap();

    println!("Faucet 1 (USDT) created: {:?}", faucet1_account.id());

    // Faucet 2: ETH
    let mut init_seed = [0u8; 32];
    client.rng().fill_bytes(&mut init_seed);

    let symbol_eth = TokenSymbol::new("ETH").unwrap();

    let key_pair_faucet2 = AuthSecretKey::new_rpo_falcon512();

    let faucet2_account = AccountBuilder::new(init_seed)
        .account_type(AccountType::FungibleFaucet)
        .storage_mode(AccountStorageMode::Public)
        .with_auth_component(AuthRpoFalcon512::new(
            key_pair_faucet2.public_key().to_commitment(),
        ))
        .with_component(BasicFungibleFaucet::new(symbol_eth, decimals, max_supply).unwrap())
        .build()
        .unwrap();

    client.add_account(&faucet2_account, false).await?;
    keystore.add_key(&key_pair_faucet2).unwrap();

    println!("Faucet 2 (ETH) created: {:?}", faucet2_account.id());

    // Sync to show newly deployed faucets
    client.sync_state().await?;
    tokio::time::sleep(Duration::from_secs(2)).await;
    println!("Both faucets deployed successfully\n");

    //------------------------------------------------------------
    // STEP 3: Create two basic wallet accounts (Alice and Bob)
    //------------------------------------------------------------
    println!("[STEP 3] Creating two basic wallet accounts");

    // Alice's account
    let mut init_seed = [0u8; 32];
    client.rng().fill_bytes(&mut init_seed);

    let key_pair_alice = AuthSecretKey::new_rpo_falcon512();

    let alice_account = AccountBuilder::new(init_seed)
        .account_type(AccountType::RegularAccountUpdatableCode)
        .storage_mode(AccountStorageMode::Private)
        .with_auth_component(AuthRpoFalcon512::new(
            key_pair_alice.public_key().to_commitment(),
        ))
        .with_component(BasicWallet)
        .build()
        .unwrap();

    client.add_account(&alice_account, false).await?;
    keystore.add_key(&key_pair_alice).unwrap();

    println!("Alice's account created: {:?}", alice_account.id());

    // Bob's account
    let mut init_seed = [0u8; 32];
    client.rng().fill_bytes(&mut init_seed);

    let key_pair_bob = AuthSecretKey::new_rpo_falcon512();

    let bob_account = AccountBuilder::new(init_seed)
        .account_type(AccountType::RegularAccountUpdatableCode)
        .storage_mode(AccountStorageMode::Private)
        .with_auth_component(AuthRpoFalcon512::new(
            key_pair_bob.public_key().to_commitment(),
        ))
        .with_component(BasicWallet)
        .build()
        .unwrap();

    client.add_account(&bob_account, false).await?;
    keystore.add_key(&key_pair_bob).unwrap();

    println!("Bob's account created: {:?}", bob_account.id());
    println!("Both wallet accounts created successfully\n");

    //------------------------------------------------------------
    // STEP 4: Mint tokens to each account
    //------------------------------------------------------------
    println!("[STEP 4] Minting tokens to each account");

    // Mint 100 USDT to Alice
    let amount_usdt: u64 = 100;
    let fungible_asset_usdt = FungibleAsset::new(faucet1_account.id(), amount_usdt).unwrap();

    let transaction_request = TransactionRequestBuilder::new()
        .build_mint_fungible_asset(
            fungible_asset_usdt,
            alice_account.id(),
            NoteType::Public,
            client.rng(),
        )
        .unwrap();

    let tx_id = client
        .submit_new_transaction(faucet1_account.id(), transaction_request)
        .await?;
    println!("Minted 100 USDT to Alice. TX: {:?}", tx_id);

    // Mint 100 ETH to Bob
    let amount_eth: u64 = 100;
    let fungible_asset_eth = FungibleAsset::new(faucet2_account.id(), amount_eth).unwrap();

    let transaction_request = TransactionRequestBuilder::new()
        .build_mint_fungible_asset(
            fungible_asset_eth,
            bob_account.id(),
            NoteType::Public,
            client.rng(),
        )
        .unwrap();

    let tx_id = client
        .submit_new_transaction(faucet2_account.id(), transaction_request)
        .await?;
    println!("Minted 100 ETH to Bob. TX: {:?}", tx_id);

    // Re-sync so minted notes become visible
    client.sync_state().await?;
    println!("Tokens minted successfully\n");

    //------------------------------------------------------------
    // STEP 5: Each account consumes their minted notes
    //------------------------------------------------------------
    println!("[STEP 5] Each account consumes their minted notes");

    // Alice consumes her USDT note
    loop {
        client.sync_state().await?;

        let consumable_notes = client
            .get_consumable_notes(Some(alice_account.id()))
            .await?;
        let list_of_note_ids: Vec<_> = consumable_notes.iter().map(|(note, _)| note.id()).collect();

        if !list_of_note_ids.is_empty() {
            println!("Alice consuming {} note(s)", list_of_note_ids.len());
            let transaction_request = TransactionRequestBuilder::new()
                .build_consume_notes(list_of_note_ids)
                .unwrap();

            let tx_id = client
                .submit_new_transaction(alice_account.id(), transaction_request)
                .await?;
            println!("Alice consumed notes. TX: {:?}", tx_id);
            break;
        } else {
            println!("Waiting for Alice's notes to be consumable...");
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    }

    // Bob consumes his ETH note
    loop {
        client.sync_state().await?;

        let consumable_notes = client.get_consumable_notes(Some(bob_account.id())).await?;
        let list_of_note_ids: Vec<_> = consumable_notes.iter().map(|(note, _)| note.id()).collect();

        if !list_of_note_ids.is_empty() {
            println!("Bob consuming {} note(s)", list_of_note_ids.len());
            let transaction_request = TransactionRequestBuilder::new()
                .build_consume_notes(list_of_note_ids)
                .unwrap();

            let tx_id = client
                .submit_new_transaction(bob_account.id(), transaction_request)
                .await?;
            println!("Bob consumed notes. TX: {:?}", tx_id);
            break;
        } else {
            println!("Waiting for Bob's notes to be consumable...");
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    }

    client.sync_state().await?;
    println!("All minted notes consumed successfully\n");

    // Wait a bit longer to ensure the consumed assets are in the vaults
    println!("Waiting for blockchain confirmations...");
    tokio::time::sleep(Duration::from_secs(10)).await;
    client.sync_state().await?;

    //------------------------------------------------------------
    // STEP 6: SCENARIO 1 - Alice creates swap note, Bob partially fills
    //------------------------------------------------------------
    println!("[STEP 6] SCENARIO 1: Alice creates swap note (Offer 50 USDT for 75 ETH)");

    // Alice creates a swap note: Offer 50 USDT for 75 ETH
    let offered_amount = 50u64;
    let requested_amount = 75u64;

    // Create the offered asset (50 USDT)
    let offered_asset = FungibleAsset::new(faucet1_account.id(), offered_amount).unwrap();
    let mut note_assets = NoteAssets::default();
    note_assets.add_asset(offered_asset.into())?;

    // Compute P2ID tag for Alice (who will receive the output note)
    let p2id_tag_felt = compute_p2id_tag_felt(alice_account.id());

    // Build note inputs (8 Felts):
    // - Positions 0-3: Requested Asset Word (Faucet2.prefix, Faucet2.suffix, 0, 75)
    // - Positions 4-6: Creator AccountId (Alice.prefix, Alice.suffix, 0)
    // - Position 7: P2ID Tag
    let note_inputs = vec![
        // Requested Asset Word (ETH)
        faucet2_account.id().prefix().into(),
        faucet2_account.id().suffix().into(),
        Felt::ZERO,
        Felt::new(requested_amount),
        // Creator AccountId (Alice)
        alice_account.id().prefix().into(),
        alice_account.id().suffix().into(),
        Felt::ZERO,
        // P2ID Tag (position 7): computed tag for Alice
        p2id_tag_felt,
    ];

    // Create the swap note using the helper function
    let swap_note = create_testing_note_from_package(
        note_package.clone(),
        alice_account.id(),
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
        .submit_new_transaction(alice_account.id(), note_publish_request)
        .await?;
    println!("Swap note published. TX: {:?}", tx_id);

    // Wait for note to be available
    tokio::time::sleep(Duration::from_secs(5)).await;
    client.sync_state().await?;
    println!("Swap note is now available\n");

    //------------------------------------------------------------
    // Bob partially fills the swap note with 40 ETH
    //------------------------------------------------------------
    println!("Bob partially fills with 40 ETH (expects ~26.67 USDT)");

    let partial_fill_amount = 40u64;

    // Wait for the swap note to be published and available
    println!("Waiting for swap note to be available...");
    tokio::time::sleep(Duration::from_secs(10)).await;
    client.sync_state().await?;

    println!("Bob consuming the swap note directly (unauthenticated)");
    println!("Note: Bob needs 40 ETH in his vault to fulfill this swap");

    // Create note args: [input_amount, 0, 0, 0]
    let note_args = Word::from([
        Felt::new(partial_fill_amount),
        Felt::ZERO,
        Felt::ZERO,
        Felt::ZERO,
    ]);

    // Calculate expected remainder amounts
    // offered_out = (input_amount * offered_total) / requested_total
    // offered_out = (40 * 50) / 75 = ~26.67
    // remainder_offered = 50 - 26.67 = ~23.33
    // remainder_requested = 75 - 40 = 35

    // Prepare expected future notes that will be created by the swap note script:
    // 1. P2ID routing note to Alice with 40 ETH (input_amount)
    // 2. Remainder swap note with remaining USDT for remaining ETH

    // Note: Computing the exact NoteDetails requires:
    // - Serial numbers (computed by script from current note's serial)
    // - Recipients (we know these)
    // - Assets (we can calculate these)
    // However, note IDs depend on the script execution, so we use empty vec
    // and let the client track notes through the transaction output
    let expected_future_notes: Vec<(NoteDetails, NoteTag)> = vec![];

    // Alternative: We could use expected_output_recipients if we had the recipients
    // but for swap notes that create dynamic P2ID and remainder notes,
    // the client will discover these from the transaction execution

    // Build consume transaction using unauthenticated_input_notes with note args
    // The swap note is public and can be consumed by anyone
    // We pass Some(note_args) to specify the input_amount
    let consume_request = TransactionRequestBuilder::new()
        .unauthenticated_input_notes([(swap_note.clone(), Some(note_args))])
        .expected_future_notes(expected_future_notes)
        .build()
        .context("Failed to build consume note transaction request")?;

    let tx_id = client
        .submit_new_transaction(bob_account.id(), consume_request)
        .await?;
    println!("Bob consumed swap note (partial fill). TX: {:?}", tx_id);

    // Expected: Bob receives ~26.67 USDT, Alice receives 40 ETH via P2ID
    // Remainder note created: 23.33 USDT for 35 ETH
    println!("Expected: Bob receives ~26 USDT, Alice receives 40 ETH");
    println!("Remainder note created: ~23 USDT for 35 ETH");

    tokio::time::sleep(Duration::from_secs(5)).await;
    client.sync_state().await?;
    println!("Partial fill completed\n");

    Ok(())

    // //------------------------------------------------------------
    // // Alice consumes the P2ID routing note (40 ETH)
    // //------------------------------------------------------------
    // println!("Alice consumes the P2ID routing note (40 ETH)");

    // loop {
    //     client.sync_state().await?;

    //     let consumable_notes = client
    //         .get_consumable_notes(Some(alice_account.id()))
    //         .await?;
    //     let list_of_note_ids: Vec<_> = consumable_notes.iter().map(|(note, _)| note.id()).collect();

    //     if !list_of_note_ids.is_empty() {
    //         println!(
    //             "Alice consuming {} note(s) (P2ID routing note)",
    //             list_of_note_ids.len()
    //         );
    //         let transaction_request = TransactionRequestBuilder::new()
    //             .build_consume_notes(list_of_note_ids)
    //             .unwrap();

    //         let tx_id = client
    //             .submit_new_transaction(alice_account.id(), transaction_request)
    //             .await?;
    //         println!("Alice consumed P2ID note. TX: {:?}", tx_id);
    //         break;
    //     } else {
    //         println!("Waiting for Alice's P2ID note...");
    //         tokio::time::sleep(Duration::from_secs(3)).await;
    //     }
    // }

    // client.sync_state().await?;
    // println!("Alice received 40 ETH successfully\n");

    // //------------------------------------------------------------
    // // STEP 7: SCENARIO 2 - Bob fully fills the remainder swap note
    // //------------------------------------------------------------
    // println!("[STEP 7] SCENARIO 2: Bob fully fills the remainder swap note (35 ETH)");

    // let full_fill_amount = 35u64;

    // // Wait for the remainder swap note to be available
    // tokio::time::sleep(Duration::from_secs(5)).await;
    // client.sync_state().await?;

    // println!("Bob consuming the remainder swap note directly");

    // // Get all notes to find the remainder swap note
    // // The remainder note is also a public swap note
    // let all_notes = client
    //     .get_input_notes(miden_client::store::NoteFilter::All)
    //     .await?;

    // // Find the remainder note (it should be the newest swap note, different from the original)
    // if let Some(remainder_note_record) = all_notes
    //     .iter()
    //     .find(|note_record| note_record.id() != swap_note.id() && note_record.metadata().is_some())
    // {
    //     println!(
    //         "Remainder swap note found: {:?}",
    //         remainder_note_record.id()
    //     );

    //     // Convert InputNoteRecord to Note for the transaction
    //     let remainder_note = remainder_note_record
    //         .clone()
    //         .try_into()
    //         .context("Failed to convert note record to note")?;

    //     // Create note args: [input_amount, 0, 0, 0]
    //     let note_args = Word::from([
    //         Felt::new(full_fill_amount),
    //         Felt::ZERO,
    //         Felt::ZERO,
    //         Felt::ZERO,
    //     ]);

    //     // For the full fill, we expect:
    //     // - P2ID routing note to Alice with 35 ETH
    //     // - No remainder note (fully consumed)
    //     let expected_future_notes: Vec<(NoteDetails, NoteTag)> = vec![];
    //     // Note: The P2ID note will be tracked automatically by the client

    //     // Build consume transaction for remainder note with note args
    //     let consume_request = TransactionRequestBuilder::new()
    //         .unauthenticated_input_notes([(remainder_note, Some(note_args))])
    //         .expected_future_notes(expected_future_notes)
    //         .build()
    //         .context("Failed to build remainder consume transaction")?;

    //     let tx_id = client
    //         .submit_new_transaction(bob_account.id(), consume_request)
    //         .await?;
    //     println!("Bob consumed remainder note (full fill). TX: {:?}", tx_id);

    //     // Expected: Bob receives ~23.33 USDT, Alice receives 35 ETH via P2ID
    //     // No remainder note (fully consumed)
    //     println!("Expected: Bob receives ~23 USDT, Alice receives 35 ETH");
    //     println!("No remainder note (swap fully completed)");
    // } else {
    //     println!("Warning: Remainder swap note not found. It may not have been created yet.");
    //     println!("This could happen if the first consumption failed or is still pending.");
    // }

    // tokio::time::sleep(Duration::from_secs(5)).await;
    // client.sync_state().await?;
    // println!("Full fill completed\n");

    // //------------------------------------------------------------
    // // Alice consumes the final P2ID routing note (35 ETH)
    // //------------------------------------------------------------
    // println!("Alice consumes the final P2ID routing note (35 ETH)");

    // loop {
    //     client.sync_state().await?;

    //     let consumable_notes = client
    //         .get_consumable_notes(Some(alice_account.id()))
    //         .await?;
    //     let list_of_note_ids: Vec<_> = consumable_notes.iter().map(|(note, _)| note.id()).collect();

    //     if !list_of_note_ids.is_empty() {
    //         println!(
    //             "Alice consuming {} note(s) (final P2ID routing note)",
    //             list_of_note_ids.len()
    //         );
    //         let transaction_request = TransactionRequestBuilder::new()
    //             .build_consume_notes(list_of_note_ids)
    //             .unwrap();

    //         let tx_id = client
    //             .submit_new_transaction(alice_account.id(), transaction_request)
    //             .await?;
    //         println!("Alice consumed final P2ID note. TX: {:?}", tx_id);
    //         break;
    //     } else {
    //         println!("Waiting for Alice's final P2ID note...");
    //         tokio::time::sleep(Duration::from_secs(3)).await;
    //     }
    // }

    // client.sync_state().await?;
    // println!("Alice received 35 ETH successfully\n");

    // //------------------------------------------------------------
    // // STEP 8: Final verification and balances
    // //------------------------------------------------------------
    // println!("[STEP 8] Final Verification");
    // println!("===========================================");
    // println!("All steps completed successfully!");
    // println!("\nExpected Final State:");
    // println!("- Alice: Started with 100 USDT, offered 50");
    // println!("  Should have: 50 USDT + 75 ETH");
    // println!("- Bob: Started with 100 ETH, spent 75");
    // println!("  Should have: 25 ETH + 50 USDT");
    // println!("\nSwap completed: 50 USDT ↔ 75 ETH");
    // println!("  - Partial fill: 40 ETH → ~26.67 USDT");
    // println!("  - Full fill: 35 ETH → ~23.33 USDT");
    // println!("===========================================");

    // Ok(())
}
