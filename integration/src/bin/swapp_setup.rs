use integration::helpers::{
    account_component_from_package, build_project_in_dir, setup_client, AccountCreationConfig,
    ClientSetup,
};
use integration::swapp_state::SwappTestState;

use anyhow::{Context, Result};
use miden_client::{
    account::component::{BasicFungibleFaucet, BasicWallet},
    auth::AuthSecretKey,
    note::NoteType,
    transaction::TransactionRequestBuilder,
    Felt,
};
use miden_lib::account::auth::AuthRpoFalcon512;
use miden_objects::{
    account::{AccountBuilder, AccountStorageMode, AccountType},
    asset::{FungibleAsset, TokenSymbol},
};
use rand::RngCore;
use std::{path::Path, sync::Arc};
use tokio::time::Duration;

/// Swap Note Setup Binary
///
/// This binary performs the one-time setup for swap note testing:
/// - Initializes client
/// - Deploys two faucets (USDT and ETH)
/// - Creates two wallet accounts (Alice and Bob)
/// - Mints tokens to each account
/// - Consumes the minted notes
///
/// After running this, the state is saved to swapp_test_state.json
/// You can then run swapp_consume binary multiple times to test swap note consumption

#[tokio::main]
async fn main() -> Result<()> {
    println!("=== Swap Note Setup ===\n");

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

    // Bob's account - using basic-wallet contract with TWO components
    println!("\nBuilding basic-wallet contract for Bob...");
    let account_package = Arc::new(
        build_project_in_dir(Path::new("contracts/basic-wallet"), true)
            .context("Failed to build basic-wallet contract")?,
    );
    println!("Basic-wallet contract built successfully.");

    // Create custom account configuration with default settings
    let bob_account_cfg = AccountCreationConfig {
        storage_slots: vec![],
        ..Default::default()
    };

    // Get the custom component from the package
    let bob_custom_component =
        account_component_from_package(account_package.clone(), &bob_account_cfg)
            .context("Failed to create Bob's account component from package")?;

    // Create Bob's account with proper auth and TWO components:
    // 1. Custom component from basic-wallet package
    // 2. BasicWallet component
    let mut init_seed = [0u8; 32];
    client.rng().fill_bytes(&mut init_seed);

    let key_pair_bob = AuthSecretKey::new_rpo_falcon512();

    let bob_account = AccountBuilder::new(init_seed)
        .account_type(AccountType::RegularAccountImmutableCode)
        .storage_mode(AccountStorageMode::Private)
        .with_auth_component(AuthRpoFalcon512::new(
            key_pair_bob.public_key().to_commitment(),
        ))
        .with_component(bob_custom_component) // Component 1: from package
        .with_component(BasicWallet) // Component 2: BasicWallet
        .build()
        .unwrap();

    client.add_account(&bob_account, false).await?;
    keystore.add_key(&key_pair_bob).unwrap();

    println!("Bob's account created: {:?}", bob_account.id());
    println!("Both wallet accounts created successfully\n");

    //------------------------------------------------------------
    // STEP 4: Mint tokens to each account (FAST SEQUENTIAL)
    //------------------------------------------------------------
    println!("[STEP 4] Minting tokens to each account");

    // Mint 100 USDT to Alice (submit immediately)
    let amount_usdt: u64 = 100000;
    let fungible_asset_usdt = FungibleAsset::new(faucet1_account.id(), amount_usdt).unwrap();
    let alice_mint_request = TransactionRequestBuilder::new()
        .build_mint_fungible_asset(
            fungible_asset_usdt,
            alice_account.id(),
            NoteType::Public,
            client.rng(),
        )
        .unwrap();
    let tx_id_alice = client
        .submit_new_transaction(faucet1_account.id(), alice_mint_request)
        .await?;
    println!("Minted 100 USDT to Alice. TX: {:?}", tx_id_alice);

    // Mint 100 ETH to Bob (submit immediately without waiting for Alice)
    let amount_eth: u64 = 100000;
    let fungible_asset_eth = FungibleAsset::new(faucet2_account.id(), amount_eth).unwrap();
    let bob_mint_request = TransactionRequestBuilder::new()
        .build_mint_fungible_asset(
            fungible_asset_eth,
            bob_account.id(),
            NoteType::Public,
            client.rng(),
        )
        .unwrap();
    let tx_id_bob = client
        .submit_new_transaction(faucet2_account.id(), bob_mint_request)
        .await?;
    println!("Minted 100 ETH to Bob. TX: {:?}", tx_id_bob);

    // Re-sync so minted notes become visible (both txs process in parallel on chain)
    client.sync_state().await?;
    println!("Tokens minted successfully (both transactions submitted quickly)\n");

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
    // Save state for the consumption test
    //------------------------------------------------------------
    println!("\n[SAVING STATE]");
    let state = SwappTestState::new(
        faucet1_account.id(),
        faucet2_account.id(),
        alice_account.id(),
        bob_account.id(),
    );
    state.save()?;

    println!("\n=== Setup Complete ===");
    println!("You can now run 'cargo run --bin swapp_consume' to test swap note consumption");

    Ok(())
}
