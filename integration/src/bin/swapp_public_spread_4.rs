use integration::helpers::{
    account_component_from_package, build_project_in_dir, setup_client, AccountCreationConfig,
    ClientSetup,
};
use integration::swapp_state::SwappTestState;

use anyhow::{Context, Result};
use miden_client::{
    account::component::BasicWallet,
    auth::AuthSecretKey,
    note::NoteType,
    transaction::{OutputNote, TransactionRequestBuilder},
    Felt, Word,
};
use miden_core::FieldElement;
use miden_protocol::{
    account::{AccountBuilder, AccountStorageMode, AccountType},
    asset::{Asset, FungibleAsset},
    note::NoteDetails,
};
use miden_standards::account::auth::AuthFalcon512Rpo;
use miden_swapp::{InflightP2idScript, PswapNote};
use rand::RngCore;
use tokio::time::Duration;

/// Public Spread Test (using miden_swapp::InflightP2idScript):
/// - Alice offers 25 USDT for 20 ETH
/// - Bob offers 20 ETH for 20 USDT
/// - Solver consumes both via InflightP2idScript, earns 5 USDT spread

#[tokio::main]
async fn main() -> Result<()> {
    println!("=== Public Spread Test (InflightP2idScript) ===\n");

    // Load persisted state
    let state = SwappTestState::load()?;
    let faucet1_id = state.faucet1_id()?; // USDT
    let faucet2_id = state.faucet2_id()?; // ETH
    let alice_id = state.alice_id()?;
    let bob_id = state.bob_id()?;

    println!("USDT Faucet: {:?}", faucet1_id);
    println!("ETH Faucet: {:?}", faucet2_id);
    println!("Alice: {:?}", alice_id);
    println!("Bob: {:?}\n", bob_id);

    // Setup client
    let ClientSetup {
        mut client,
        keystore,
    } = setup_client().await?;
    client.sync_state().await?;

    // Create Solver account with custom basic-wallet component
    println!("Creating Solver account with custom basic-wallet...");
    let account_package = std::sync::Arc::new(
        build_project_in_dir(std::path::Path::new("contracts/basic-wallet"), true)
            .context("Failed to build basic-wallet contract")?,
    );

    let solver_account_cfg = AccountCreationConfig {
        storage_slots: vec![],
        ..Default::default()
    };

    let solver_custom_component =
        account_component_from_package(account_package.clone(), &solver_account_cfg)
            .context("Failed to create Solver's account component")?;

    let mut init_seed = [0u8; 32];
    client.rng().fill_bytes(&mut init_seed);

    let key_pair_solver = AuthSecretKey::new_falcon512_rpo();

    let solver_account = AccountBuilder::new(init_seed)
        .account_type(AccountType::RegularAccountImmutableCode)
        .storage_mode(AccountStorageMode::Public)
        .with_auth_component(AuthFalcon512Rpo::new(
            key_pair_solver.public_key().to_commitment(),
        ))
        .with_component(solver_custom_component)
        .with_component(BasicWallet)
        .build()
        .unwrap();

    client.add_account(&solver_account, false).await?;
    keystore.add_key(&key_pair_solver).unwrap();

    let solver_id = solver_account.id();
    println!("Solver: {:?}\n", solver_id);
    client.sync_state().await?;

    //------------------------------------------------------------
    // Alice creates swap note: 25 USDT for 20 ETH
    //------------------------------------------------------------
    println!("[1] Alice creates swap note (25 USDT -> 20 ETH)");

    let alice_swap_note = PswapNote::create(
        alice_id,
        Asset::Fungible(FungibleAsset::new(faucet1_id, 25)?),
        Asset::Fungible(FungibleAsset::new(faucet2_id, 20)?),
        NoteType::Public,
        miden_protocol::note::NoteAttachment::default(),
        client.rng(),
    )
    .map_err(|e| anyhow::anyhow!("Failed to create Alice's swap note: {:?}", e))?;

    let tx_id = client
        .submit_new_transaction(
            alice_id,
            TransactionRequestBuilder::new()
                .own_output_notes(vec![OutputNote::Full(alice_swap_note.clone())])
                .build()
                .unwrap(),
        )
        .await?;
    println!("Published. TX: {:?}", tx_id);

    //------------------------------------------------------------
    // Bob creates swap note: 20 ETH for 20 USDT
    //------------------------------------------------------------
    println!("\n[2] Bob creates swap note (20 ETH -> 20 USDT)");

    let bob_swap_note = PswapNote::create(
        bob_id,
        Asset::Fungible(FungibleAsset::new(faucet2_id, 20)?),
        Asset::Fungible(FungibleAsset::new(faucet1_id, 20)?),
        NoteType::Public,
        miden_protocol::note::NoteAttachment::default(),
        client.rng(),
    )
    .map_err(|e| anyhow::anyhow!("Failed to create Bob's swap note: {:?}", e))?;

    let tx_id = client
        .submit_new_transaction(
            bob_id,
            TransactionRequestBuilder::new()
                .own_output_notes(vec![OutputNote::Full(bob_swap_note.clone())])
                .build()
                .unwrap(),
        )
        .await?;
    println!("Published. TX: {:?}", tx_id);

    // Wait for notes
    println!("\nWaiting for notes to be available...");
    tokio::time::sleep(Duration::from_secs(10)).await;
    client.sync_state().await?;
    client.sync_state().await?;

    //------------------------------------------------------------
    // Solver consumes both notes via InflightP2idScript
    //------------------------------------------------------------
    println!("\n[3] Solver consuming both swap notes (with InflightP2idScript for spread)");

    // Note args: arg[0]=input, arg[1]=inflight (swapp-note only reads these two)
    let alice_note_args = Word::from([
        Felt::ZERO,
        Felt::ZERO,
        Felt::new(20), // inflight = 20 ETH
        Felt::ZERO,    // input = 0
    ]);

    let bob_note_args = Word::from([
        Felt::ZERO,
        Felt::ZERO,
        Felt::new(20), // inflight = 20 USDT
        Felt::ZERO,
    ]);

    // P2ID for Alice (20 ETH) and Bob (20 USDT) via PswapNote
    let (alice_p2id_note, _) = PswapNote::create_output_notes(&alice_swap_note, solver_id, 0, 20)
        .map_err(|e| anyhow::anyhow!("Alice P2ID: {:?}", e))?;

    let (bob_p2id_note, _) = PswapNote::create_output_notes(&bob_swap_note, solver_id, 0, 20)
        .map_err(|e| anyhow::anyhow!("Bob P2ID: {:?}", e))?;

    // Use InflightP2idScript to get the tx script and prepare the solver's spread note
    let tx_script = InflightP2idScript::tx_script();

    let solver_spread_asset = Asset::Fungible(FungibleAsset::new(faucet1_id, 5)?);
    let data =
        InflightP2idScript::prepare(solver_id, &[(solver_spread_asset, solver_id)], client.rng())
            .map_err(|e| anyhow::anyhow!("InflightP2idScript::prepare failed: {:?}", e))?;

    let solver_p2id_note = &data.expected_notes[0];

    println!("Alice P2ID: {:?}", alice_p2id_note.id());
    println!("Bob P2ID: {:?}", bob_p2id_note.id());
    println!("Solver P2ID: {:?}", solver_p2id_note.id());

    // Build expected future notes
    let expected_future_notes = vec![
        (
            NoteDetails::from(&alice_p2id_note),
            alice_p2id_note.metadata().tag(),
        ),
        (
            NoteDetails::from(&bob_p2id_note),
            bob_p2id_note.metadata().tag(),
        ),
        (
            NoteDetails::from(solver_p2id_note),
            solver_p2id_note.metadata().tag(),
        ),
    ];

    // Submit consume transaction with InflightP2idScript
    let consume_request = TransactionRequestBuilder::new()
        .input_notes(vec![
            (alice_swap_note.clone(), Some(alice_note_args)),
            (bob_swap_note.clone(), Some(bob_note_args)),
        ])
        .custom_script(tx_script)
        .script_arg(data.commitment_arg)
        .extend_advice_map([data.advice_map_entry])
        .expected_future_notes(expected_future_notes)
        .expected_output_recipients(vec![
            alice_p2id_note.recipient().clone(),
            bob_p2id_note.recipient().clone(),
            solver_p2id_note.recipient().clone(),
        ])
        .build()
        .context("Failed to build consume transaction")?;

    let tx_id = client
        .submit_new_transaction(solver_id, consume_request)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to execute cross-swap transaction: {:?}", e))?;
    println!("\nSolver consumed both notes. TX: {:?}", tx_id);

    println!("Waiting for processing...");
    tokio::time::sleep(Duration::from_secs(60)).await;
    client.sync_state().await?;

    //------------------------------------------------------------
    // Each party consumes their P2ID note
    //------------------------------------------------------------
    println!("\n[4] Solver consuming P2ID (5 USDT spread)");
    match client
        .submit_new_transaction(
            solver_id,
            TransactionRequestBuilder::new()
                .input_notes(vec![(solver_p2id_note.clone(), None)])
                .build()?,
        )
        .await
    {
        Ok(id) => println!("SUCCESS TX: {:?}", id),
        Err(e) => println!("FAILED: {:?}", e),
    }

    tokio::time::sleep(Duration::from_secs(5)).await;
    client.sync_state().await?;

    println!("\n[5] Alice consuming P2ID (20 ETH)");
    match client
        .submit_new_transaction(
            alice_id,
            TransactionRequestBuilder::new()
                .input_notes(vec![(alice_p2id_note.clone(), None)])
                .build()?,
        )
        .await
    {
        Ok(id) => println!("SUCCESS TX: {:?}", id),
        Err(e) => println!("FAILED: {:?}", e),
    }

    tokio::time::sleep(Duration::from_secs(5)).await;
    client.sync_state().await?;

    println!("\n[6] Bob consuming P2ID (20 USDT)");
    match client
        .submit_new_transaction(
            bob_id,
            TransactionRequestBuilder::new()
                .input_notes(vec![(bob_p2id_note.clone(), None)])
                .build()?,
        )
        .await
    {
        Ok(id) => println!("SUCCESS TX: {:?}", id),
        Err(e) => println!("FAILED: {:?}", e),
    }

    println!("\n=== Test Complete ===");
    println!("Alice: 25 USDT -> 20 ETH");
    println!("Bob: 20 ETH -> 20 USDT");
    println!("Solver: 5 USDT spread profit (via InflightP2idScript)");

    Ok(())
}
