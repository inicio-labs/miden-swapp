mod api;
mod note_pool;
mod solver;

use anyhow::{Context, Result};
use integration::helpers::{
    account_component_from_package, build_project_in_dir, setup_client, AccountCreationConfig,
    ClientSetup,
};
use miden_client::{account::component::BasicWallet, auth::AuthSecretKey};
use miden_lib::account::auth::AuthRpoFalcon512;
use miden_objects::account::{AccountBuilder, AccountStorageMode, AccountType};
use rand::RngCore;
use std::{path::Path, sync::Arc};

use note_pool::NotePool;

/// Create Bob's solver account with BasicWallet and custom component
async fn create_solver_account(
    client_setup: &mut ClientSetup,
) -> Result<miden_client::account::Account> {
    let ClientSetup { client, keystore } = client_setup;

    println!("   Building basic-wallet contract for Bob...");
    let account_package = Arc::new(
        build_project_in_dir(Path::new("contracts/basic-wallet"), true)
            .context("Failed to build basic-wallet contract")?,
    );
    println!("   ✓ Basic-wallet contract built");

    // Create custom account configuration
    let bob_account_cfg = AccountCreationConfig {
        storage_slots: vec![],
        ..Default::default()
    };

    // Get the custom component from the package
    let bob_custom_component =
        account_component_from_package(account_package.clone(), &bob_account_cfg)
            .context("Failed to create Bob's account component from package")?;

    // Create Bob's account with auth and two components
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
        .context("Failed to build Bob's account")?;

    client
        .add_account(&bob_account, false)
        .await
        .context("Failed to add Bob's account to client")?;

    keystore
        .add_key(&key_pair_bob)
        .context("Failed to add Bob's key to keystore")?;

    Ok(bob_account)
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("\n🚀 ========================================");
    println!("   MIDEN SWAP COORDINATOR SERVER");
    println!("========================================\n");

    // Initialize Miden client
    println!("🔧 Initializing Miden client...");
    let mut client_setup = setup_client().await?;
    println!("✓ Client initialized\n");

    // Sync to get latest state
    println!("🔄 Syncing with network...");
    client_setup.client.sync_state().await?;
    println!("✓ Network synced\n");

    // Create Bob solver account
    println!("👤 Creating Bob solver account...");
    let bob_account = create_solver_account(&mut client_setup).await?;

    // *** PROMINENT LOGGING OF BOB'S ACCOUNT ***
    println!("\n========================================");
    println!("🤖 BOB SOLVER ACCOUNT CREATED");
    println!("========================================");
    println!("Account ID: {:?}", bob_account.id());
    println!("Account ID (hex): {}", bob_account.id().to_hex());
    println!("========================================");
    println!("\nℹ️  Please fund this account with tokens");
    println!("   to enable Bob to fulfill swaps.");
    println!("========================================\n");

    // Initialize note pool
    println!("💾 Initializing note pool...");
    let note_pool = NotePool::new();
    println!("✓ Note pool ready\n");

    // Spawn Bob solver loop in a LocalSet (allows !Send futures)
    println!("🔄 Starting Bob solver background loop...");
    let bob_account_id = bob_account.id();
    let solver_client = client_setup.client;
    let solver_note_pool = note_pool.clone();

    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        solver::run_solver_loop(solver_client, bob_account_id, solver_note_pool).await;
    });
    println!("✓ Bob solver running (scans every 5 seconds)\n");

    // Start HTTP server
    println!("🌐 Starting HTTP API server on 0.0.0.0:3000...");
    let app = api::create_api_routes(note_pool);
    println!("✓ Server ready!");
    println!("\n📡 API Endpoints:");
    println!("   POST   http://localhost:3000/notes");
    println!("   GET    http://localhost:3000/notes/:id\n");
    println!("========================================\n");

    // Bind and serve
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000")
        .await
        .context("Failed to bind to port 3000")?;

    // Run both the server and the local tasks together
    local
        .run_until(async move { axum::serve(listener, app).await.context("Server error") })
        .await?;

    Ok(())
}
