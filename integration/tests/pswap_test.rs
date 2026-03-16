use integration::helpers::build_project_in_dir;

use miden_client::{
    note::{
        Note, NoteAssets, NoteInputs, NoteMetadata, NoteRecipient, NoteScript, NoteTag, NoteType,
    },
    transaction::OutputNote,
    Felt, Word,
};
use miden_core::FieldElement;
use miden_crypto::hash::rpo::Rpo256;
use miden_protocol::{
    account::AccountId,
    asset::{Asset, FungibleAsset},
    note::{NoteAttachment, NoteAttachmentScheme},
    transaction::TransactionScript,
};
use miden_standards::{code_builder::CodeBuilder, note::utils::build_p2id_recipient};
use miden_testing::{Auth, MockChain};
use std::{collections::BTreeMap, path::Path, sync::Arc};

/// Path to pswap.masm source file
fn pswap_masm_path() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("asm")
        .join("pswap.masm")
}

/// Compile pswap.masm into a NoteScript using CodeBuilder from miden-standards
fn compile_pswap_note_script() -> NoteScript {
    let masm_path = pswap_masm_path();
    let masm_source = std::fs::read_to_string(&masm_path)
        .unwrap_or_else(|e| panic!("Failed to read pswap.masm at {:?}: {}", masm_path, e));

    CodeBuilder::new()
        .compile_note_script(masm_source.as_str())
        .expect("Failed to compile pswap.masm")
}

/// Compute the P2ID tag for a local account
fn compute_p2id_tag_for_local_account(account_id: AccountId) -> NoteTag {
    NoteTag::with_account_target(account_id)
}

/// Helper function to compute P2ID tag as Felt for use in note inputs
fn compute_p2id_tag_felt(account_id: AccountId) -> Felt {
    let p2id_tag = compute_p2id_tag_for_local_account(account_id);
    Felt::new(u32::from(p2id_tag) as u64)
}

/// Create a PSWAP note with 14-input layout
fn create_pswap_note(
    note_script: NoteScript,
    sender_id: AccountId,
    note_assets: NoteAssets,
    note_inputs: Vec<Felt>,
    note_tag: NoteTag,
) -> Note {
    create_pswap_note_with_type(
        note_script,
        sender_id,
        note_assets,
        note_inputs,
        note_tag,
        NoteType::Public,
    )
}

/// Create a PSWAP note with specified note type
fn create_pswap_note_with_type(
    note_script: NoteScript,
    sender_id: AccountId,
    note_assets: NoteAssets,
    note_inputs: Vec<Felt>,
    note_tag: NoteTag,
    note_type: NoteType,
) -> Note {
    use miden_crypto::rand::{FeltRng, RpoRandomCoin};
    let mut rng = RpoRandomCoin::new(Word::default());
    let serial_num = rng.draw_word();

    let note_inputs = NoteInputs::new(note_inputs).expect("Failed to create note inputs");
    let recipient = NoteRecipient::new(serial_num, note_script, note_inputs);
    let metadata = NoteMetadata::new(sender_id, note_type, note_tag);

    Note::new(note_assets, metadata, recipient)
}

/// Calculate output amount matching PSWAP.masm's calculate_tokens_offered_for_requested
/// Uses u64 integer arithmetic with precision_factor=100000
fn calculate_output_amount(offered_total: u64, requested_total: u64, input_amount: u64) -> u64 {
    let precision_factor = 100000u64;
    if offered_total > requested_total {
        let ratio = (offered_total * precision_factor) / requested_total;
        (input_amount * ratio) / precision_factor
    } else {
        let ratio = (requested_total * precision_factor) / offered_total;
        (input_amount * precision_factor) / ratio
    }
}

/// Build 14-input vector for a PSWAP note
fn build_pswap_inputs(
    requested_faucet_id: AccountId,
    requested_amount: u64,
    swapp_tag_felt: Felt,
    p2id_tag_felt: Felt,
    swap_count: u64,
    creator_id: AccountId,
) -> Vec<Felt> {
    vec![
        requested_faucet_id.prefix().into(),
        requested_faucet_id.suffix(),
        Felt::ZERO,
        Felt::new(requested_amount),
        swapp_tag_felt,
        p2id_tag_felt,
        Felt::ZERO,
        Felt::ZERO,
        Felt::new(swap_count),
        Felt::ZERO,
        Felt::ZERO,
        Felt::ZERO,
        creator_id.prefix().into(),
        creator_id.suffix(),
    ]
}

/// Create expected P2ID note for a PSWAP note consumption
fn create_expected_pswap_p2id_note(
    swap_note: &Note,
    consumer_id: AccountId,
    creator_id: AccountId,
    swap_count: u64,
    total_fill: u64,
    requested_faucet_id: AccountId,
    p2id_tag: NoteTag,
) -> anyhow::Result<Note> {
    // Derive note_type from the swap note's metadata (matches PSWAP.masm behavior)
    let note_type = swap_note.metadata().note_type();
    create_expected_pswap_p2id_note_with_type(
        swap_note,
        consumer_id,
        creator_id,
        swap_count,
        total_fill,
        requested_faucet_id,
        p2id_tag,
        note_type,
    )
}

/// Create expected P2ID note with explicit note type
fn create_expected_pswap_p2id_note_with_type(
    swap_note: &Note,
    consumer_id: AccountId,
    creator_id: AccountId,
    swap_count: u64,
    total_fill: u64,
    requested_faucet_id: AccountId,
    p2id_tag: NoteTag,
    note_type: NoteType,
) -> anyhow::Result<Note> {
    let swap_count_word = Word::from([
        Felt::new(swap_count + 1),
        Felt::ZERO,
        Felt::ZERO,
        Felt::ZERO,
    ]);
    let serial_num = swap_note.recipient().serial_num();
    let p2id_serial = Rpo256::merge(&[serial_num.into(), swap_count_word.into()]);
    let p2id_serial_word: Word = Word::from(p2id_serial);

    let p2id_recipient = build_p2id_recipient(creator_id, p2id_serial_word)?;
    let p2id_asset = FungibleAsset::new(requested_faucet_id, total_fill)?;
    let p2id_note_assets = NoteAssets::new(vec![p2id_asset.into()])?;

    let aux = Felt::new(total_fill);
    let aux_word = Word::from([aux, Felt::ZERO, Felt::ZERO, Felt::ZERO]);
    let attachment = NoteAttachment::new_word(NoteAttachmentScheme::none(), aux_word);
    let metadata = NoteMetadata::new(consumer_id, note_type, p2id_tag).with_attachment(attachment);

    Ok(Note::new(p2id_note_assets, metadata, p2id_recipient))
}

/// Create NoteAssets with a single fungible asset
fn make_note_assets(faucet_id: AccountId, amount: u64) -> anyhow::Result<NoteAssets> {
    let asset = FungibleAsset::new(faucet_id, amount)?;
    Ok(NoteAssets::new(vec![asset.into()])?)
}

/// Create the standard SWAPp tag and its Felt representation
fn make_swapp_tag() -> (NoteTag, Felt) {
    let tag = NoteTag::new(0xC0000000);
    let felt = Felt::new(u32::from(tag) as u64);
    (tag, felt)
}

/// Build note args Word from input and inflight amounts
fn make_note_args(input_amount: u64, inflight_amount: u64) -> Word {
    Word::from([
        Felt::ZERO,
        Felt::ZERO,
        Felt::new(inflight_amount),
        Felt::new(input_amount),
    ])
}

/// Create expected SWAPp remainder note for a PSWAP partial fill
fn create_expected_pswap_remainder_note(
    swap_note: &Note,
    pswap_script: NoteScript,
    consumer_id: AccountId,
    creator_id: AccountId,
    remaining_offered: u64,
    remaining_requested: u64,
    offered_out: u64,
    swap_count: u64,
    offered_faucet_id: AccountId,
    requested_faucet_id: AccountId,
    swapp_tag: NoteTag,
    swapp_tag_felt: Felt,
    p2id_tag_felt: Felt,
) -> anyhow::Result<Note> {
    let serial_num = swap_note.recipient().serial_num();
    let remainder_serial = Word::from([
        serial_num[0],
        serial_num[1],
        serial_num[2],
        Felt::new(serial_num[3].as_int() + 1),
    ]);

    let remainder_inputs = build_pswap_inputs(
        requested_faucet_id,
        remaining_requested,
        swapp_tag_felt,
        p2id_tag_felt,
        swap_count + 1,
        creator_id,
    );

    let remainder_inputs_obj = NoteInputs::new(remainder_inputs)?;
    let remainder_recipient =
        NoteRecipient::new(remainder_serial, pswap_script, remainder_inputs_obj);

    let remainder_asset = FungibleAsset::new(offered_faucet_id, remaining_offered)?;
    let remainder_note_assets = NoteAssets::new(vec![remainder_asset.into()])?;

    let aux = Felt::new(offered_out);
    let aux_word = Word::from([aux, Felt::ZERO, Felt::ZERO, Felt::ZERO]);
    let attachment = NoteAttachment::new_word(NoteAttachmentScheme::none(), aux_word);
    let metadata =
        NoteMetadata::new(consumer_id, NoteType::Public, swapp_tag).with_attachment(attachment);

    Ok(Note::new(
        remainder_note_assets,
        metadata,
        remainder_recipient,
    ))
}

#[tokio::test]
async fn pswap_note_full_fill_test() -> anyhow::Result<()> {
    let mut builder = MockChain::builder();

    let usdc_faucet =
        builder.add_existing_basic_faucet(Auth::BasicAuth, "USDC", 1000, Some(150))?;
    let eth_faucet = builder.add_existing_basic_faucet(Auth::BasicAuth, "ETH", 1000, Some(50))?;

    let alice = builder.add_existing_wallet_with_assets(
        Auth::BasicAuth,
        [FungibleAsset::new(usdc_faucet.id(), 50)?.into()],
    )?;
    let bob = builder.add_existing_wallet_with_assets(
        Auth::BasicAuth,
        [FungibleAsset::new(eth_faucet.id(), 25)?.into()],
    )?;

    let pswap_script = compile_pswap_note_script();
    let (swapp_tag, swapp_tag_felt) = make_swapp_tag();
    let p2id_tag_felt = compute_p2id_tag_felt(alice.id());

    let note_inputs = build_pswap_inputs(
        eth_faucet.id(),
        25,
        swapp_tag_felt,
        p2id_tag_felt,
        0,
        alice.id(),
    );
    let note_assets = make_note_assets(usdc_faucet.id(), 50)?;
    let swap_note = create_pswap_note(
        pswap_script.clone(),
        alice.id(),
        note_assets,
        note_inputs,
        swapp_tag,
    );
    builder.add_output_note(OutputNote::Full(swap_note.clone()));

    let mut mock_chain = builder.build()?;

    let mut note_args_map = BTreeMap::new();
    note_args_map.insert(swap_note.id(), make_note_args(25, 0));

    let p2id_note = create_expected_pswap_p2id_note(
        &swap_note,
        bob.id(),
        alice.id(),
        0,
        25,
        eth_faucet.id(),
        compute_p2id_tag_for_local_account(alice.id()),
    )?;

    let tx_context = mock_chain
        .build_tx_context(bob.id(), &[swap_note.id()], &[])?
        .extend_note_args(note_args_map)
        .extend_expected_output_notes(vec![OutputNote::Full(p2id_note.clone())])
        .build()?;

    let executed_transaction = tx_context.execute().await?;

    // Verify: 1 P2ID note with 25 ETH
    let output_notes = executed_transaction.output_notes();
    assert_eq!(output_notes.num_notes(), 1, "Expected exactly 1 P2ID note");

    let actual_recipient = output_notes.get_note(0).recipient_digest().unwrap();
    let expected_recipient = p2id_note.recipient().digest();
    println!("ACTUAL   recipient from VM:   {:?}", actual_recipient);
    println!("EXPECTED recipient from Rust: {:?}", expected_recipient);
    println!("P2ID serial we computed:      {:?}", p2id_note.recipient().serial_num());
    println!("P2ID script root:             {:?}", p2id_note.recipient().script().root());
    println!("P2ID inputs commitment:       {:?}", p2id_note.recipient().inputs().commitment());
    assert_eq!(actual_recipient, expected_recipient, "RECIPIENT MISMATCH!");

    let p2id_assets = output_notes.get_note(0).assets().unwrap();
    assert_eq!(p2id_assets.num_assets(), 1);
    if let Asset::Fungible(f) = p2id_assets.iter().next().unwrap() {
        assert_eq!(f.faucet_id(), eth_faucet.id());
        assert_eq!(f.amount(), 25);
    } else {
        panic!("Expected fungible asset in P2ID note");
    }

    // Verify Bob's vault delta: +50 USDC, -25 ETH
    let vault_delta = executed_transaction.account_delta().vault();
    let added: Vec<Asset> = vault_delta.added_assets().collect();
    let removed: Vec<Asset> = vault_delta.removed_assets().collect();

    assert_eq!(added.len(), 1);
    assert_eq!(removed.len(), 1);
    if let Asset::Fungible(f) = &added[0] {
        assert_eq!(f.faucet_id(), usdc_faucet.id());
        assert_eq!(f.amount(), 50);
    }
    if let Asset::Fungible(f) = &removed[0] {
        assert_eq!(f.faucet_id(), eth_faucet.id());
        assert_eq!(f.amount(), 25);
    }

    mock_chain.add_pending_executed_transaction(&executed_transaction)?;
    let _ = mock_chain.prove_next_block();

    Ok(())
}

#[tokio::test]
async fn pswap_note_private_full_fill_test() -> anyhow::Result<()> {
    let mut builder = MockChain::builder();

    let usdc_faucet =
        builder.add_existing_basic_faucet(Auth::BasicAuth, "USDC", 1000, Some(150))?;
    let eth_faucet = builder.add_existing_basic_faucet(Auth::BasicAuth, "ETH", 1000, Some(50))?;

    let alice = builder.add_existing_wallet_with_assets(
        Auth::BasicAuth,
        [FungibleAsset::new(usdc_faucet.id(), 50)?.into()],
    )?;
    let bob = builder.add_existing_wallet_with_assets(
        Auth::BasicAuth,
        [FungibleAsset::new(eth_faucet.id(), 25)?.into()],
    )?;

    let pswap_script = compile_pswap_note_script();
    let (swapp_tag, swapp_tag_felt) = make_swapp_tag();
    let p2id_tag_felt = compute_p2id_tag_felt(alice.id());

    let note_inputs = build_pswap_inputs(
        eth_faucet.id(),
        25,
        swapp_tag_felt,
        p2id_tag_felt,
        0,
        alice.id(),
    );
    let note_assets = make_note_assets(usdc_faucet.id(), 50)?;

    // Create a PRIVATE swap note (output notes should also be Private)
    let swap_note = create_pswap_note_with_type(
        pswap_script.clone(),
        alice.id(),
        note_assets,
        note_inputs,
        swapp_tag,
        NoteType::Private,
    );
    builder.add_output_note(OutputNote::Full(swap_note.clone()));

    let mut mock_chain = builder.build()?;

    let mut note_args_map = BTreeMap::new();
    note_args_map.insert(swap_note.id(), make_note_args(25, 0));

    // Expected P2ID note should inherit Private type from swap note
    let p2id_note = create_expected_pswap_p2id_note_with_type(
        &swap_note,
        bob.id(),
        alice.id(),
        0,
        25,
        eth_faucet.id(),
        compute_p2id_tag_for_local_account(alice.id()),
        NoteType::Private,
    )?;

    let tx_context = mock_chain
        .build_tx_context(bob.id(), &[swap_note.id()], &[])?
        .extend_note_args(note_args_map)
        .extend_expected_output_notes(vec![OutputNote::Full(p2id_note)])
        .build()?;

    let executed_transaction = tx_context.execute().await?;

    // Verify: 1 P2ID note with 25 ETH
    let output_notes = executed_transaction.output_notes();
    assert_eq!(output_notes.num_notes(), 1, "Expected exactly 1 P2ID note");

    let p2id_assets = output_notes.get_note(0).assets().unwrap();
    assert_eq!(p2id_assets.num_assets(), 1);
    if let Asset::Fungible(f) = p2id_assets.iter().next().unwrap() {
        assert_eq!(f.faucet_id(), eth_faucet.id());
        assert_eq!(f.amount(), 25);
    } else {
        panic!("Expected fungible asset in P2ID note");
    }

    // Verify Bob's vault delta: +50 USDC, -25 ETH
    let vault_delta = executed_transaction.account_delta().vault();
    let added: Vec<Asset> = vault_delta.added_assets().collect();
    let removed: Vec<Asset> = vault_delta.removed_assets().collect();

    assert_eq!(added.len(), 1);
    assert_eq!(removed.len(), 1);
    if let Asset::Fungible(f) = &added[0] {
        assert_eq!(f.faucet_id(), usdc_faucet.id());
        assert_eq!(f.amount(), 50);
    }
    if let Asset::Fungible(f) = &removed[0] {
        assert_eq!(f.faucet_id(), eth_faucet.id());
        assert_eq!(f.amount(), 25);
    }

    mock_chain.add_pending_executed_transaction(&executed_transaction)?;
    let _ = mock_chain.prove_next_block();

    Ok(())
}

#[tokio::test]
async fn pswap_note_partial_fill_test() -> anyhow::Result<()> {
    let mut builder = MockChain::builder();

    let usdc_faucet =
        builder.add_existing_basic_faucet(Auth::BasicAuth, "USDC", 1000, Some(150))?;
    let eth_faucet = builder.add_existing_basic_faucet(Auth::BasicAuth, "ETH", 1000, Some(50))?;

    let alice = builder.add_existing_wallet_with_assets(
        Auth::BasicAuth,
        [FungibleAsset::new(usdc_faucet.id(), 50)?.into()],
    )?;
    let bob = builder.add_existing_wallet_with_assets(
        Auth::BasicAuth,
        [FungibleAsset::new(eth_faucet.id(), 20)?.into()],
    )?;

    let pswap_script = compile_pswap_note_script();
    let (swapp_tag, swapp_tag_felt) = make_swapp_tag();
    let p2id_tag_felt = compute_p2id_tag_felt(alice.id());

    let note_inputs = build_pswap_inputs(
        eth_faucet.id(),
        25,
        swapp_tag_felt,
        p2id_tag_felt,
        0,
        alice.id(),
    );
    let note_assets = make_note_assets(usdc_faucet.id(), 50)?;
    let swap_note = create_pswap_note(
        pswap_script.clone(),
        alice.id(),
        note_assets,
        note_inputs,
        swapp_tag,
    );
    builder.add_output_note(OutputNote::Full(swap_note.clone()));

    let mut mock_chain = builder.build()?;

    let mut note_args_map = BTreeMap::new();
    note_args_map.insert(swap_note.id(), make_note_args(20, 0));

    // Expected P2ID note: 20 ETH for Alice
    let p2id_note = create_expected_pswap_p2id_note(
        &swap_note,
        bob.id(),
        alice.id(),
        0,
        20,
        eth_faucet.id(),
        compute_p2id_tag_for_local_account(alice.id()),
    )?;

    // Expected SWAPp remainder: 10 USDC for 5 ETH (offered_out=40, remaining=50-40=10)
    let remainder_note = create_expected_pswap_remainder_note(
        &swap_note,
        pswap_script.clone(),
        bob.id(),
        alice.id(),
        10,
        5,
        40,
        0,
        usdc_faucet.id(),
        eth_faucet.id(),
        swapp_tag,
        swapp_tag_felt,
        p2id_tag_felt,
    )?;

    let tx_context = mock_chain
        .build_tx_context(bob.id(), &[swap_note.id()], &[])?
        .extend_note_args(note_args_map)
        .extend_expected_output_notes(vec![
            OutputNote::Full(p2id_note),
            OutputNote::Full(remainder_note),
        ])
        .build()?;

    let executed_transaction = tx_context.execute().await?;

    // Verify: 2 output notes (P2ID + remainder)
    let output_notes = executed_transaction.output_notes();
    assert_eq!(output_notes.num_notes(), 2);

    // P2ID note: 20 ETH
    if let Asset::Fungible(f) = output_notes
        .get_note(0)
        .assets()
        .unwrap()
        .iter()
        .next()
        .unwrap()
    {
        assert_eq!(f.faucet_id(), eth_faucet.id());
        assert_eq!(f.amount(), 20);
    }

    // SWAPp remainder: 10 USDC
    if let Asset::Fungible(f) = output_notes
        .get_note(1)
        .assets()
        .unwrap()
        .iter()
        .next()
        .unwrap()
    {
        assert_eq!(f.faucet_id(), usdc_faucet.id());
        assert_eq!(f.amount(), 10);
    }

    // Bob's vault: +40 USDC, -20 ETH
    let vault_delta = executed_transaction.account_delta().vault();
    let added: Vec<Asset> = vault_delta.added_assets().collect();
    let removed: Vec<Asset> = vault_delta.removed_assets().collect();
    assert_eq!(added.len(), 1);
    assert_eq!(removed.len(), 1);
    if let Asset::Fungible(f) = &added[0] {
        assert_eq!(f.faucet_id(), usdc_faucet.id());
        assert_eq!(f.amount(), 40);
    }
    if let Asset::Fungible(f) = &removed[0] {
        assert_eq!(f.faucet_id(), eth_faucet.id());
        assert_eq!(f.amount(), 20);
    }

    mock_chain.add_pending_executed_transaction(&executed_transaction)?;
    let _ = mock_chain.prove_next_block();

    Ok(())
}

#[tokio::test]
async fn pswap_note_inflight_cross_swap_test() -> anyhow::Result<()> {
    let mut builder = MockChain::builder();

    let usdc_faucet =
        builder.add_existing_basic_faucet(Auth::BasicAuth, "USDC", 1000, Some(150))?;
    let eth_faucet = builder.add_existing_basic_faucet(Auth::BasicAuth, "ETH", 1000, Some(50))?;

    let alice = builder.add_existing_wallet_with_assets(
        Auth::BasicAuth,
        [FungibleAsset::new(usdc_faucet.id(), 50)?.into()],
    )?;
    let bob = builder.add_existing_wallet_with_assets(
        Auth::BasicAuth,
        [FungibleAsset::new(eth_faucet.id(), 25)?.into()],
    )?;

    let charlie = builder.add_existing_wallet_with_assets(Auth::BasicAuth, [])?;

    let pswap_script = compile_pswap_note_script();
    let (swapp_tag, swapp_tag_felt) = make_swapp_tag();

    // Alice's note: offers 50 USDC, requests 25 ETH
    let alice_inputs = build_pswap_inputs(
        eth_faucet.id(),
        25,
        swapp_tag_felt,
        compute_p2id_tag_felt(alice.id()),
        0,
        alice.id(),
    );
    let alice_swap_note = create_pswap_note(
        pswap_script.clone(),
        alice.id(),
        make_note_assets(usdc_faucet.id(), 50)?,
        alice_inputs,
        swapp_tag,
    );
    builder.add_output_note(OutputNote::Full(alice_swap_note.clone()));

    // Bob's note: offers 25 ETH, requests 50 USDC
    let bob_inputs = build_pswap_inputs(
        usdc_faucet.id(),
        50,
        swapp_tag_felt,
        compute_p2id_tag_felt(bob.id()),
        0,
        bob.id(),
    );
    let bob_swap_note = create_pswap_note(
        pswap_script.clone(),
        bob.id(),
        make_note_assets(eth_faucet.id(), 25)?,
        bob_inputs,
        swapp_tag,
    );
    builder.add_output_note(OutputNote::Full(bob_swap_note.clone()));

    let mock_chain = builder.build()?;

    // Note args: pure inflight (input=0, inflight=full amount)
    let mut note_args_map = BTreeMap::new();
    note_args_map.insert(alice_swap_note.id(), make_note_args(0, 25));
    note_args_map.insert(bob_swap_note.id(), make_note_args(0, 50));

    // Expected P2ID notes
    let alice_p2id_note = create_expected_pswap_p2id_note(
        &alice_swap_note,
        charlie.id(),
        alice.id(),
        0,
        25,
        eth_faucet.id(),
        compute_p2id_tag_for_local_account(alice.id()),
    )?;
    let bob_p2id_note = create_expected_pswap_p2id_note(
        &bob_swap_note,
        charlie.id(),
        bob.id(),
        0,
        50,
        usdc_faucet.id(),
        compute_p2id_tag_for_local_account(bob.id()),
    )?;

    let tx_context = mock_chain
        .build_tx_context(
            charlie.id(),
            &[alice_swap_note.id(), bob_swap_note.id()],
            &[],
        )?
        .extend_note_args(note_args_map)
        .extend_expected_output_notes(vec![
            OutputNote::Full(alice_p2id_note),
            OutputNote::Full(bob_p2id_note),
        ])
        .build()?;

    let executed_transaction = tx_context.execute().await?;

    // Verify: 2 P2ID notes
    let output_notes = executed_transaction.output_notes();
    assert_eq!(output_notes.num_notes(), 2);

    let mut alice_found = false;
    let mut bob_found = false;
    for idx in 0..output_notes.num_notes() {
        if let Asset::Fungible(f) = output_notes
            .get_note(idx)
            .assets()
            .unwrap()
            .iter()
            .next()
            .unwrap()
        {
            if f.faucet_id() == eth_faucet.id() && f.amount() == 25 {
                alice_found = true;
            }
            if f.faucet_id() == usdc_faucet.id() && f.amount() == 50 {
                bob_found = true;
            }
        }
    }
    assert!(alice_found, "Alice's P2ID note (25 ETH) not found");
    assert!(bob_found, "Bob's P2ID note (50 USDC) not found");

    // Charlie's vault should be unchanged
    let vault_delta = executed_transaction.account_delta().vault();
    assert_eq!(vault_delta.added_assets().count(), 0);
    assert_eq!(vault_delta.removed_assets().count(), 0);

    Ok(())
}

#[tokio::test]
async fn pswap_note_creator_reclaim_test() -> anyhow::Result<()> {
    println!("=== Test: PSWAP Creator Reclaim ===");
    let mut builder = MockChain::builder();

    // Create faucets
    let usdc_faucet = builder.add_existing_basic_faucet(Auth::BasicAuth, "USDC", 1000, Some(50))?;
    let eth_faucet = builder.add_existing_basic_faucet(Auth::BasicAuth, "ETH", 1000, Some(25))?;

    // Alice: creator with custom wallet (needed because she's also the consumer)
    let alice = builder.add_existing_wallet_with_assets(
        Auth::BasicAuth,
        [FungibleAsset::new(usdc_faucet.id(), 50)?.into()],
    )?;

    let pswap_script = compile_pswap_note_script();
    let (swapp_tag, swapp_tag_felt) = make_swapp_tag();
    let p2id_tag_felt = compute_p2id_tag_felt(alice.id());

    let note_inputs = build_pswap_inputs(
        eth_faucet.id(),
        25,
        swapp_tag_felt,
        p2id_tag_felt,
        0,
        alice.id(),
    );
    let swap_note = create_pswap_note(
        pswap_script,
        alice.id(),
        make_note_assets(usdc_faucet.id(), 50)?,
        note_inputs,
        swapp_tag,
    );
    builder.add_output_note(OutputNote::Full(swap_note.clone()));

    let mock_chain = builder.build()?;

    let tx_context = mock_chain
        .build_tx_context(alice.id(), &[swap_note.id()], &[])?
        .build()?;

    let executed_transaction = tx_context.execute().await?;

    // Verify: 0 output notes, Alice gets 50 USDC back
    let output_notes = executed_transaction.output_notes();
    assert_eq!(
        output_notes.num_notes(),
        0,
        "Expected 0 output notes for reclaim"
    );

    let account_delta = executed_transaction.account_delta();
    let vault_delta = account_delta.vault();
    let added_assets: Vec<Asset> = vault_delta.added_assets().collect();

    assert_eq!(added_assets.len(), 1, "Alice should receive 1 asset back");
    let usdc_reclaimed = match added_assets[0] {
        Asset::Fungible(f) => f,
        _ => panic!("Expected fungible USDC asset"),
    };
    assert_eq!(usdc_reclaimed.faucet_id(), usdc_faucet.id());
    assert_eq!(usdc_reclaimed.amount(), 50);
    println!("Alice reclaimed 50 USDC. PSWAP creator reclaim test passed!");

    Ok(())
}

#[tokio::test]
async fn pswap_note_invalid_input_test() -> anyhow::Result<()> {
    println!("=== Test: PSWAP Invalid Input (input > requested) ===");
    let mut builder = MockChain::builder();

    let usdc_faucet = builder.add_existing_basic_faucet(Auth::BasicAuth, "USDC", 1000, Some(50))?;
    let eth_faucet = builder.add_existing_basic_faucet(Auth::BasicAuth, "ETH", 1000, Some(30))?;

    let alice = builder.add_existing_wallet_with_assets(
        Auth::BasicAuth,
        [FungibleAsset::new(usdc_faucet.id(), 50)?.into()],
    )?;

    let bob = builder.add_existing_wallet_with_assets(
        Auth::BasicAuth,
        [FungibleAsset::new(eth_faucet.id(), 30)?.into()],
    )?;

    let pswap_script = compile_pswap_note_script();
    let (swapp_tag, swapp_tag_felt) = make_swapp_tag();
    let p2id_tag_felt = compute_p2id_tag_felt(alice.id());

    let note_inputs = build_pswap_inputs(
        eth_faucet.id(),
        25,
        swapp_tag_felt,
        p2id_tag_felt,
        0,
        alice.id(),
    );
    let swap_note = create_pswap_note(
        pswap_script,
        alice.id(),
        make_note_assets(usdc_faucet.id(), 50)?,
        note_inputs,
        swapp_tag,
    );
    builder.add_output_note(OutputNote::Full(swap_note.clone()));
    let mock_chain = builder.build()?;

    let mut note_args_map = BTreeMap::new();
    note_args_map.insert(swap_note.id(), make_note_args(30, 0));

    let tx_context = mock_chain
        .build_tx_context(bob.id(), &[swap_note.id()], &[])?
        .extend_note_args(note_args_map)
        .build()?;

    let result = tx_context.execute().await;
    assert!(
        result.is_err(),
        "Transaction should fail when input_amount > requested_asset_total"
    );
    println!("Transaction correctly failed. PSWAP invalid input test passed!");

    Ok(())
}

#[tokio::test]
async fn pswap_note_multiple_partial_fills_test() -> anyhow::Result<()> {
    println!("=== Test: PSWAP Multiple Partial Fill Scenarios ===");

    let test_scenarios = vec![
        (5u64, "5 ETH - 20% fill"),
        (7, "7 ETH - 28% fill"),
        (10, "10 ETH - 40% fill"),
        (13, "13 ETH - 52% fill"),
        (15, "15 ETH - 60% fill"),
        (19, "19 ETH - 76% fill"),
        (20, "20 ETH - 80% fill"),
        (23, "23 ETH - 92% fill"),
        (25, "25 ETH - 100% fill (full)"),
    ];

    let pswap_script = compile_pswap_note_script();

    for (input_amount, description) in test_scenarios {
        println!("\nTesting scenario: {}", description);

        let mut builder = MockChain::builder();
        let usdc_faucet =
            builder.add_existing_basic_faucet(Auth::BasicAuth, "USDC", 1000, Some(150))?;
        let eth_faucet =
            builder.add_existing_basic_faucet(Auth::BasicAuth, "ETH", 1000, Some(50))?;

        let alice = builder.add_existing_wallet_with_assets(
            Auth::BasicAuth,
            [FungibleAsset::new(usdc_faucet.id(), 50)?.into()],
        )?;

        let bob = builder.add_existing_wallet_with_assets(
            Auth::BasicAuth,
            [FungibleAsset::new(eth_faucet.id(), input_amount)?.into()],
        )?;

        let (swapp_tag, swapp_tag_felt) = make_swapp_tag();
        let p2id_tag_felt = compute_p2id_tag_felt(alice.id());

        let note_inputs = build_pswap_inputs(
            eth_faucet.id(),
            25,
            swapp_tag_felt,
            p2id_tag_felt,
            0,
            alice.id(),
        );
        let swap_note = create_pswap_note(
            pswap_script.clone(),
            alice.id(),
            make_note_assets(usdc_faucet.id(), 50)?,
            note_inputs,
            swapp_tag,
        );
        builder.add_output_note(OutputNote::Full(swap_note.clone()));

        let mock_chain = builder.build()?;

        let offered_out = calculate_output_amount(50, 25, input_amount);
        let remaining_usdc = 50 - offered_out;
        let remaining_eth = 25 - input_amount;

        let mut note_args_map = BTreeMap::new();
        note_args_map.insert(swap_note.id(), make_note_args(input_amount, 0));

        // Build expected P2ID note
        let p2id_note = create_expected_pswap_p2id_note(
            &swap_note,
            bob.id(),
            alice.id(),
            0,
            input_amount,
            eth_faucet.id(),
            compute_p2id_tag_for_local_account(alice.id()),
        )?;

        let mut expected_notes = vec![OutputNote::Full(p2id_note)];

        // Build expected remainder note (only if partial fill)
        if input_amount < 25 {
            let remainder_note = create_expected_pswap_remainder_note(
                &swap_note,
                pswap_script.clone(),
                bob.id(),
                alice.id(),
                remaining_usdc,
                remaining_eth,
                offered_out,
                0,
                usdc_faucet.id(),
                eth_faucet.id(),
                swapp_tag,
                swapp_tag_felt,
                p2id_tag_felt,
            )?;
            expected_notes.push(OutputNote::Full(remainder_note));
        }

        let tx_context = mock_chain
            .build_tx_context(bob.id(), &[swap_note.id()], &[])?
            .extend_expected_output_notes(expected_notes)
            .extend_note_args(note_args_map)
            .build()?;

        let executed_transaction = tx_context.execute().await?;

        let output_notes = executed_transaction.output_notes();
        let expected_count = if input_amount < 25 { 2 } else { 1 };
        assert_eq!(output_notes.num_notes(), expected_count);

        // Verify P2ID note
        let mut p2id_found = false;
        let mut remainder_found = false;
        for idx in 0..output_notes.num_notes() {
            let note = output_notes.get_note(idx);
            let assets = note.assets().unwrap();
            if let Asset::Fungible(f) = assets.iter().next().unwrap() {
                if f.faucet_id() == eth_faucet.id() {
                    assert_eq!(f.amount(), input_amount);
                    p2id_found = true;
                } else if f.faucet_id() == usdc_faucet.id() {
                    assert_eq!(f.amount(), remaining_usdc);
                    remainder_found = true;
                }
            }
        }
        assert!(p2id_found, "P2ID note not found");
        if input_amount < 25 {
            assert!(remainder_found, "Remainder note not found");
        }

        // Verify Bob's vault
        let vault_delta = executed_transaction.account_delta().vault();
        let added: Vec<Asset> = vault_delta.added_assets().collect();
        assert_eq!(added.len(), 1);
        if let Asset::Fungible(f) = added[0] {
            assert_eq!(f.amount(), offered_out);
        }

        println!("  Scenario '{}' passed!", description);
    }

    println!("\nAll PSWAP multiple partial fill scenarios passed!");
    Ok(())
}

#[tokio::test]
async fn pswap_note_inflight_cross_swap_with_spread_test() -> anyhow::Result<()> {
    println!("=== Test: PSWAP Inflight Cross Swap With Spread (Charlie Earns 5 ETH) ===");
    println!("Alice offers 30 ETH for 50 USDC, Bob offers 50 USDC for 25 ETH");
    println!("Spread: 30 - 25 = 5 ETH to Charlie");
    let mut builder = MockChain::builder();

    let usdc_faucet =
        builder.add_existing_basic_faucet(Auth::BasicAuth, "USDC", 1000, Some(100))?;
    let eth_faucet = builder.add_existing_basic_faucet(Auth::BasicAuth, "ETH", 1000, Some(50))?;

    // Alice: offers 30 ETH, requests 50 USDC
    let alice = builder.add_existing_wallet_with_assets(
        Auth::BasicAuth,
        [FungibleAsset::new(eth_faucet.id(), 30)?.into()],
    )?;

    // Bob: offers 50 USDC, requests 25 ETH
    let bob = builder.add_existing_wallet_with_assets(
        Auth::BasicAuth,
        [FungibleAsset::new(usdc_faucet.id(), 50)?.into()],
    )?;

    // Charlie: 0 assets, facilitator who earns spread
    let charlie = builder.add_existing_wallet_with_assets(Auth::BasicAuth, [])?;

    let pswap_script = compile_pswap_note_script();
    let swapp_tag = NoteTag::new(0xC0000000);
    let swapp_tag_felt = Felt::new(u32::from(swapp_tag) as u64);

    // Alice's note: offers 30 ETH, requests 50 USDC
    let alice_note_inputs = build_pswap_inputs(
        usdc_faucet.id(),
        50,
        swapp_tag_felt,
        compute_p2id_tag_felt(alice.id()),
        0,
        alice.id(),
    );
    let mut alice_note_assets = NoteAssets::default();
    alice_note_assets.add_asset(FungibleAsset::new(eth_faucet.id(), 30)?.into())?;
    let alice_swap_note = create_pswap_note(
        pswap_script.clone(),
        alice.id(),
        alice_note_assets,
        alice_note_inputs,
        swapp_tag,
    );
    builder.add_output_note(OutputNote::Full(alice_swap_note.clone()));

    // Bob's note: offers 50 USDC, requests 25 ETH
    let bob_note_inputs = build_pswap_inputs(
        eth_faucet.id(),
        25,
        swapp_tag_felt,
        compute_p2id_tag_felt(bob.id()),
        0,
        bob.id(),
    );
    let mut bob_note_assets = NoteAssets::default();
    bob_note_assets.add_asset(FungibleAsset::new(usdc_faucet.id(), 50)?.into())?;
    let bob_swap_note = create_pswap_note(
        pswap_script.clone(),
        bob.id(),
        bob_note_assets,
        bob_note_inputs,
        swapp_tag,
    );
    builder.add_output_note(OutputNote::Full(bob_swap_note.clone()));

    // Build consume-asset-script for spread
    let consume_asset_package = Arc::new(build_project_in_dir(
        Path::new("../contracts/consume-asset-script"),
        true,
    )?);
    let program = consume_asset_package.unwrap_program();
    let tx_script =
        TransactionScript::from_parts(program.mast_forest().clone(), program.entrypoint());

    let mock_chain = builder.build()?;

    // Note args: pure inflight
    let alice_note_args = Word::from([Felt::ZERO, Felt::ZERO, Felt::new(50), Felt::ZERO]);
    let bob_note_args = Word::from([Felt::ZERO, Felt::ZERO, Felt::new(25), Felt::ZERO]);

    let mut note_args_map = BTreeMap::new();
    note_args_map.insert(alice_swap_note.id(), alice_note_args);
    note_args_map.insert(bob_swap_note.id(), bob_note_args);

    // Expected P2ID notes
    let alice_p2id_note = create_expected_pswap_p2id_note(
        &alice_swap_note,
        charlie.id(),
        alice.id(),
        0,
        50,
        usdc_faucet.id(),
        compute_p2id_tag_for_local_account(alice.id()),
    )?;
    let bob_p2id_note = create_expected_pswap_p2id_note(
        &bob_swap_note,
        charlie.id(),
        bob.id(),
        0,
        25,
        eth_faucet.id(),
        compute_p2id_tag_for_local_account(bob.id()),
    )?;

    // Advice stack for spread (5 ETH consumed directly into Charlie's vault)
    let spread_asset_word = Word::from(Asset::from(FungibleAsset::new(eth_faucet.id(), 5)?));
    let advice_stack: Vec<Felt> = spread_asset_word.to_vec();

    let commitment_key: Word = Rpo256::hash_elements(&advice_stack);
    let mut commitment = commitment_key;
    commitment.reverse();

    let tx_context = mock_chain
        .build_tx_context(
            charlie.id(),
            &[alice_swap_note.id(), bob_swap_note.id()],
            &[],
        )?
        .tx_script(tx_script)
        .tx_script_args(commitment)
        .extend_advice_map([(commitment_key, advice_stack)])
        .extend_expected_output_notes(vec![
            OutputNote::Full(alice_p2id_note),
            OutputNote::Full(bob_p2id_note),
        ])
        .extend_note_args(note_args_map)
        .build()?;

    let executed_transaction = tx_context.execute().await?;

    // Verify: 2 P2ID notes
    let output_notes = executed_transaction.output_notes();
    assert_eq!(output_notes.num_notes(), 2);

    let mut alice_found = false;
    let mut bob_found = false;
    for idx in 0..output_notes.num_notes() {
        let note = output_notes.get_note(idx);
        let assets = note.assets().unwrap();
        if let Asset::Fungible(f) = assets.iter().next().unwrap() {
            if f.faucet_id() == usdc_faucet.id() && f.amount() == 50 {
                alice_found = true;
            } else if f.faucet_id() == eth_faucet.id() && f.amount() == 25 {
                bob_found = true;
            }
        }
    }
    assert!(alice_found, "Alice's P2ID note (50 USDC) not found");
    assert!(bob_found, "Bob's P2ID note (25 ETH) not found");

    // Verify Charlie's vault: +5 ETH spread
    let vault_delta = executed_transaction.account_delta().vault();
    let added_assets: Vec<Asset> = vault_delta.added_assets().collect();
    let removed_assets: Vec<Asset> = vault_delta.removed_assets().collect();

    assert_eq!(removed_assets.len(), 0);
    assert_eq!(added_assets.len(), 1);
    if let Asset::Fungible(f) = &added_assets[0] {
        assert_eq!(f.faucet_id(), eth_faucet.id());
        assert_eq!(f.amount(), 5);
    }
    println!("PSWAP inflight cross-swap with spread test passed! Charlie earned 5 ETH.");

    Ok(())
}

#[tokio::test]
async fn pswap_note_inflight_cross_swap_with_usdt_spread_test() -> anyhow::Result<()> {
    println!("=== Test: PSWAP Inflight Cross Swap With USDT Spread ===");
    println!("Alice offers 23 USDT for 20 ETH, Bob offers 20 ETH for 18 USDT");
    println!("Spread: 23 - 18 = 5 USDT to Charlie");
    let mut builder = MockChain::builder();

    let usdt_faucet =
        builder.add_existing_basic_faucet(Auth::BasicAuth, "USDT", 1000, Some(230))?;
    let eth_faucet = builder.add_existing_basic_faucet(Auth::BasicAuth, "ETH", 1000, Some(200))?;

    let alice = builder.add_existing_wallet_with_assets(
        Auth::BasicAuth,
        [FungibleAsset::new(usdt_faucet.id(), 23)?.into()],
    )?;
    let bob = builder.add_existing_wallet_with_assets(
        Auth::BasicAuth,
        [FungibleAsset::new(eth_faucet.id(), 20)?.into()],
    )?;

    let charlie = builder.add_existing_wallet_with_assets(Auth::BasicAuth, [])?;

    let pswap_script = compile_pswap_note_script();
    let swapp_tag = NoteTag::new(0xC0000000);
    let swapp_tag_felt = Felt::new(u32::from(swapp_tag) as u64);

    // Alice's note: offers 23 USDT, requests 20 ETH
    let alice_note_inputs = build_pswap_inputs(
        eth_faucet.id(),
        20,
        swapp_tag_felt,
        compute_p2id_tag_felt(alice.id()),
        0,
        alice.id(),
    );
    let mut alice_assets = NoteAssets::default();
    alice_assets.add_asset(FungibleAsset::new(usdt_faucet.id(), 23)?.into())?;
    let alice_swap_note = create_pswap_note(
        pswap_script.clone(),
        alice.id(),
        alice_assets,
        alice_note_inputs,
        swapp_tag,
    );
    builder.add_output_note(OutputNote::Full(alice_swap_note.clone()));

    // Bob's note: offers 20 ETH, requests 18 USDT
    let bob_note_inputs = build_pswap_inputs(
        usdt_faucet.id(),
        18,
        swapp_tag_felt,
        compute_p2id_tag_felt(bob.id()),
        0,
        bob.id(),
    );
    let mut bob_assets = NoteAssets::default();
    bob_assets.add_asset(FungibleAsset::new(eth_faucet.id(), 20)?.into())?;
    let bob_swap_note = create_pswap_note(
        pswap_script.clone(),
        bob.id(),
        bob_assets,
        bob_note_inputs,
        swapp_tag,
    );
    builder.add_output_note(OutputNote::Full(bob_swap_note.clone()));

    // Build consume-asset-script for spread
    let consume_asset_package = Arc::new(build_project_in_dir(
        Path::new("../contracts/consume-asset-script"),
        true,
    )?);
    let program = consume_asset_package.unwrap_program();
    let tx_script =
        TransactionScript::from_parts(program.mast_forest().clone(), program.entrypoint());

    let mock_chain = builder.build()?;

    let alice_note_args = Word::from([Felt::ZERO, Felt::ZERO, Felt::new(20), Felt::ZERO]);
    let bob_note_args = Word::from([Felt::ZERO, Felt::ZERO, Felt::new(18), Felt::ZERO]);

    let mut note_args_map = BTreeMap::new();
    note_args_map.insert(alice_swap_note.id(), alice_note_args);
    note_args_map.insert(bob_swap_note.id(), bob_note_args);

    let alice_p2id_note = create_expected_pswap_p2id_note(
        &alice_swap_note,
        charlie.id(),
        alice.id(),
        0,
        20,
        eth_faucet.id(),
        compute_p2id_tag_for_local_account(alice.id()),
    )?;
    let bob_p2id_note = create_expected_pswap_p2id_note(
        &bob_swap_note,
        charlie.id(),
        bob.id(),
        0,
        18,
        usdt_faucet.id(),
        compute_p2id_tag_for_local_account(bob.id()),
    )?;

    let spread_asset_word = Word::from(Asset::from(FungibleAsset::new(usdt_faucet.id(), 5)?));
    let advice_stack: Vec<Felt> = spread_asset_word.to_vec();
    let commitment_key: Word = Rpo256::hash_elements(&advice_stack);
    let mut commitment = commitment_key;
    commitment.reverse();

    let tx_context = mock_chain
        .build_tx_context(
            charlie.id(),
            &[alice_swap_note.id(), bob_swap_note.id()],
            &[],
        )?
        .tx_script(tx_script)
        .tx_script_args(commitment)
        .extend_advice_map([(commitment_key, advice_stack)])
        .extend_expected_output_notes(vec![
            OutputNote::Full(alice_p2id_note),
            OutputNote::Full(bob_p2id_note),
        ])
        .extend_note_args(note_args_map)
        .build()?;

    let executed_transaction = tx_context.execute().await?;

    let output_notes = executed_transaction.output_notes();
    assert_eq!(output_notes.num_notes(), 2);

    let mut alice_found = false;
    let mut bob_found = false;
    for idx in 0..output_notes.num_notes() {
        let note = output_notes.get_note(idx);
        let assets = note.assets().unwrap();
        if let Asset::Fungible(f) = assets.iter().next().unwrap() {
            if f.faucet_id() == eth_faucet.id() && f.amount() == 20 {
                alice_found = true;
            } else if f.faucet_id() == usdt_faucet.id() && f.amount() == 18 {
                bob_found = true;
            }
        }
    }
    assert!(alice_found, "Alice's P2ID note (20 ETH) not found");
    assert!(bob_found, "Bob's P2ID note (18 USDT) not found");

    let vault_delta = executed_transaction.account_delta().vault();
    let added_assets: Vec<Asset> = vault_delta.added_assets().collect();
    assert_eq!(added_assets.len(), 1);
    if let Asset::Fungible(f) = &added_assets[0] {
        assert_eq!(f.faucet_id(), usdt_faucet.id());
        assert_eq!(f.amount(), 5);
    }
    println!("PSWAP USDT spread test passed! Charlie earned 5 USDT.");

    Ok(())
}

#[tokio::test]
async fn pswap_note_inflight_cross_swap_fuzz_test() -> anyhow::Result<()> {
    println!("=== Fuzz Test: PSWAP Inflight Cross Swap With Spread ===\n");

    let consume_asset_package = Arc::new(build_project_in_dir(
        Path::new("../contracts/consume-asset-script"),
        true,
    )?);

    let pswap_script = compile_pswap_note_script();

    // (alice_offered_eth, alice_requested_usdc, bob_requested_eth)
    // alice_eth > bob_eth (positive spread)
    let mut test_cases: Vec<(u64, u64, u64)> = vec![
        (2, 1, 1),
        (3, 1, 1),
        (2, 100, 1),
        (10, 10, 9),
        (50, 1, 49),
        (100, 99, 99),
        (100, 100, 1),
        (99, 50, 1),
        (7, 5, 3),
        (17, 13, 11),
        (53, 47, 41),
        (97, 89, 83),
        (23, 21, 17),
        (24, 21, 17),
        (51, 49, 47),
        (4, 3, 2),
        (64, 32, 16),
        (128, 64, 32),
        (10, 10, 5),
        (20, 10, 15),
        (10, 20, 5),
        (4, 5, 2),
        (6, 10, 3),
    ];

    use rand::Rng;
    let mut thread_rng = rand::rng();
    for _ in 0..20 {
        let alice_eth: u64 = thread_rng.random_range(2..500);
        let usdc: u64 = thread_rng.random_range(1..500);
        let bob_eth: u64 = thread_rng.random_range(1..alice_eth);
        test_cases.push((alice_eth, usdc, bob_eth));
    }

    let total = test_cases.len();
    println!("Running {} test cases...\n", total);

    for (i, (alice_eth, usdc, bob_eth)) in test_cases.iter().enumerate() {
        let spread = alice_eth - bob_eth;
        println!(
            "--- Case {}/{}: Alice {}ETH for {}USDC | Bob {}USDC for {}ETH | spread={}ETH ---",
            i + 1,
            total,
            alice_eth,
            usdc,
            usdc,
            bob_eth,
            spread
        );

        let mut builder = MockChain::builder();
        let max_amount = *[*alice_eth, *usdc].iter().max().unwrap();
        let total_issuance = max_amount * 10;

        let usdc_faucet = builder.add_existing_basic_faucet(
            Auth::BasicAuth,
            "USDC",
            1000,
            Some(total_issuance),
        )?;
        let eth_faucet = builder.add_existing_basic_faucet(
            Auth::BasicAuth,
            "ETH",
            1000,
            Some(total_issuance),
        )?;

        let alice = builder.add_existing_wallet_with_assets(
            Auth::BasicAuth,
            [FungibleAsset::new(eth_faucet.id(), *alice_eth)?.into()],
        )?;
        let bob = builder.add_existing_wallet_with_assets(
            Auth::BasicAuth,
            [FungibleAsset::new(usdc_faucet.id(), *usdc)?.into()],
        )?;

        let charlie = builder.add_existing_wallet_with_assets(Auth::BasicAuth, [])?;

        let swapp_tag = NoteTag::new(0xC0000000);
        let swapp_tag_felt = Felt::new(u32::from(swapp_tag) as u64);

        // Alice's note: offers alice_eth ETH, requests usdc USDC
        let alice_inputs = build_pswap_inputs(
            usdc_faucet.id(),
            *usdc,
            swapp_tag_felt,
            compute_p2id_tag_felt(alice.id()),
            0,
            alice.id(),
        );
        let mut alice_assets = NoteAssets::default();
        alice_assets.add_asset(FungibleAsset::new(eth_faucet.id(), *alice_eth)?.into())?;
        let alice_note = create_pswap_note(
            pswap_script.clone(),
            alice.id(),
            alice_assets,
            alice_inputs,
            swapp_tag,
        );
        builder.add_output_note(OutputNote::Full(alice_note.clone()));

        // Bob's note: offers usdc USDC, requests bob_eth ETH
        let bob_inputs = build_pswap_inputs(
            eth_faucet.id(),
            *bob_eth,
            swapp_tag_felt,
            compute_p2id_tag_felt(bob.id()),
            0,
            bob.id(),
        );
        let mut bob_assets = NoteAssets::default();
        bob_assets.add_asset(FungibleAsset::new(usdc_faucet.id(), *usdc)?.into())?;
        let bob_note = create_pswap_note(
            pswap_script.clone(),
            bob.id(),
            bob_assets,
            bob_inputs,
            swapp_tag,
        );
        builder.add_output_note(OutputNote::Full(bob_note.clone()));

        let program = consume_asset_package.unwrap_program();
        let tx_script =
            TransactionScript::from_parts(program.mast_forest().clone(), program.entrypoint());

        let mock_chain = builder.build()?;

        let alice_args = Word::from([Felt::ZERO, Felt::ZERO, Felt::new(*usdc), Felt::ZERO]);
        let bob_args = Word::from([Felt::ZERO, Felt::ZERO, Felt::new(*bob_eth), Felt::ZERO]);
        let mut note_args_map = BTreeMap::new();
        note_args_map.insert(alice_note.id(), alice_args);
        note_args_map.insert(bob_note.id(), bob_args);

        let alice_p2id = create_expected_pswap_p2id_note(
            &alice_note,
            charlie.id(),
            alice.id(),
            0,
            *usdc,
            usdc_faucet.id(),
            compute_p2id_tag_for_local_account(alice.id()),
        )?;
        let bob_p2id = create_expected_pswap_p2id_note(
            &bob_note,
            charlie.id(),
            bob.id(),
            0,
            *bob_eth,
            eth_faucet.id(),
            compute_p2id_tag_for_local_account(bob.id()),
        )?;

        let spread_word = Word::from(Asset::from(FungibleAsset::new(eth_faucet.id(), spread)?));
        let advice_stack: Vec<Felt> = spread_word.to_vec();
        let commitment_key: Word = Rpo256::hash_elements(&advice_stack);
        let mut commitment = commitment_key;
        commitment.reverse();

        let tx_context = mock_chain
            .build_tx_context(charlie.id(), &[alice_note.id(), bob_note.id()], &[])?
            .tx_script(tx_script)
            .tx_script_args(commitment)
            .extend_advice_map([(commitment_key, advice_stack)])
            .extend_expected_output_notes(vec![
                OutputNote::Full(alice_p2id),
                OutputNote::Full(bob_p2id),
            ])
            .extend_note_args(note_args_map)
            .build()?;

        let executed_tx = tx_context.execute().await?;

        let output_notes = executed_tx.output_notes();
        assert_eq!(
            output_notes.num_notes(),
            2,
            "Case {}: Expected 2 P2ID notes",
            i + 1
        );

        let vault_delta = executed_tx.account_delta().vault();
        assert_eq!(
            vault_delta.removed_assets().count(),
            0,
            "Case {}: No removals",
            i + 1
        );
        let added: Vec<Asset> = vault_delta.added_assets().collect();
        assert_eq!(added.len(), 1, "Case {}: 1 spread asset", i + 1);
        if let Asset::Fungible(f) = &added[0] {
            assert_eq!(f.amount(), spread, "Case {}: spread={}", i + 1, spread);
        }

        println!(
            "  PASSED: Alice {} USDC, Bob {} ETH, Charlie {} ETH spread",
            usdc, bob_eth, spread
        );
    }

    println!("\n=== All {} PSWAP fuzz test cases passed! ===", total);
    Ok(())
}

#[tokio::test]
async fn pswap_note_cross_swap_ultimate_fuzz_test() -> anyhow::Result<()> {
    println!("=== Ultimate Fuzz Test: PSWAP Independent Offer/Request Cross Swap ===\n");

    let consume_asset_package = Arc::new(build_project_in_dir(
        Path::new("../contracts/consume-asset-script"),
        true,
    )?);
    let pswap_script = compile_pswap_note_script();

    // (alice_offer_eth, alice_request_usdc, bob_offer_usdc, bob_request_eth)
    let mut test_cases: Vec<(u64, u64, u64, u64)> = vec![
        (23, 20, 20, 18),
        (17, 13, 13, 11),
        (7, 5, 5, 3),
        (97, 89, 89, 83),
        (53, 47, 47, 41),
        (1000, 1, 1, 999),
        (2, 999, 999, 1),
        (500, 3, 7, 499),
        (50, 30, 30, 49),
        (50, 30, 31, 50),
        (2, 1, 1, 1),
        (1, 1, 2, 1),
        (2, 1, 2, 1),
        (3, 2, 3, 2),
        (9999, 7777, 8888, 5555),
        (5000, 4999, 5001, 4998),
        (256, 128, 128, 64),
        (1024, 512, 768, 256),
        (51, 49, 50, 48),
        (101, 100, 100, 99),
        (11, 10, 10, 9),
        (100, 100, 100, 50),
        (100, 50, 100, 100),
        (100, 100, 100, 99),
        (3, 7, 7, 2),
        (7, 3, 3, 6),
        (11, 7, 7, 10),
        (89, 55, 55, 34),
        (233, 144, 144, 89),
        (34, 21, 21, 13),
    ];

    use rand::Rng;
    let mut thread_rng = rand::rng();
    for _ in 0..30 {
        let ao: u64 = thread_rng.random_range(2..1000);
        let ar: u64 = thread_rng.random_range(1..1000);
        let co: u64 = thread_rng.random_range(ar..=ar + 500);
        let cr: u64 = thread_rng.random_range(1..ao);
        test_cases.push((ao, ar, co, cr));
    }
    test_cases.retain(|(ao, ar, co, cr)| ao > cr || co > ar);

    let total = test_cases.len();
    println!("Running {} test cases...\n", total);

    for (i, (alice_offer, alice_request, bob_offer, bob_request)) in test_cases.iter().enumerate() {
        let eth_spread = alice_offer - bob_request;
        let usdc_spread = bob_offer - alice_request;

        println!(
            "--- Case {}/{}: Alice {}ETH->{}USDC | Bob {}USDC->{}ETH | spread: {}ETH+{}USDC ---",
            i + 1,
            total,
            alice_offer,
            alice_request,
            bob_offer,
            bob_request,
            eth_spread,
            usdc_spread,
        );

        let mut builder = MockChain::builder();
        let max_amount = *[*alice_offer, *alice_request, *bob_offer, *bob_request]
            .iter()
            .max()
            .unwrap();
        let total_issuance = max_amount * 10;

        let usdc_faucet = builder.add_existing_basic_faucet(
            Auth::BasicAuth,
            "USDC",
            1000,
            Some(total_issuance),
        )?;
        let eth_faucet = builder.add_existing_basic_faucet(
            Auth::BasicAuth,
            "ETH",
            1000,
            Some(total_issuance),
        )?;

        let alice = builder.add_existing_wallet_with_assets(
            Auth::BasicAuth,
            [FungibleAsset::new(eth_faucet.id(), *alice_offer)?.into()],
        )?;
        let bob = builder.add_existing_wallet_with_assets(
            Auth::BasicAuth,
            [FungibleAsset::new(usdc_faucet.id(), *bob_offer)?.into()],
        )?;

        let charlie = builder.add_existing_wallet_with_assets(Auth::BasicAuth, [])?;

        let swapp_tag = NoteTag::new(0xC0000000);
        let swapp_tag_felt = Felt::new(u32::from(swapp_tag) as u64);

        let alice_inputs = build_pswap_inputs(
            usdc_faucet.id(),
            *alice_request,
            swapp_tag_felt,
            compute_p2id_tag_felt(alice.id()),
            0,
            alice.id(),
        );
        let mut alice_assets = NoteAssets::default();
        alice_assets.add_asset(FungibleAsset::new(eth_faucet.id(), *alice_offer)?.into())?;
        let alice_note = create_pswap_note(
            pswap_script.clone(),
            alice.id(),
            alice_assets,
            alice_inputs,
            swapp_tag,
        );
        builder.add_output_note(OutputNote::Full(alice_note.clone()));

        let bob_inputs = build_pswap_inputs(
            eth_faucet.id(),
            *bob_request,
            swapp_tag_felt,
            compute_p2id_tag_felt(bob.id()),
            0,
            bob.id(),
        );
        let mut bob_assets = NoteAssets::default();
        bob_assets.add_asset(FungibleAsset::new(usdc_faucet.id(), *bob_offer)?.into())?;
        let bob_note = create_pswap_note(
            pswap_script.clone(),
            bob.id(),
            bob_assets,
            bob_inputs,
            swapp_tag,
        );
        builder.add_output_note(OutputNote::Full(bob_note.clone()));

        let program = consume_asset_package.unwrap_program();
        let tx_script =
            TransactionScript::from_parts(program.mast_forest().clone(), program.entrypoint());

        let mock_chain = builder.build()?;

        let alice_args = Word::from([
            Felt::ZERO,
            Felt::ZERO,
            Felt::new(*alice_request),
            Felt::ZERO,
        ]);
        let bob_args = Word::from([Felt::ZERO, Felt::ZERO, Felt::new(*bob_request), Felt::ZERO]);
        let mut note_args_map = BTreeMap::new();
        note_args_map.insert(alice_note.id(), alice_args);
        note_args_map.insert(bob_note.id(), bob_args);

        let alice_p2id = create_expected_pswap_p2id_note(
            &alice_note,
            charlie.id(),
            alice.id(),
            0,
            *alice_request,
            usdc_faucet.id(),
            compute_p2id_tag_for_local_account(alice.id()),
        )?;
        let bob_p2id = create_expected_pswap_p2id_note(
            &bob_note,
            charlie.id(),
            bob.id(),
            0,
            *bob_request,
            eth_faucet.id(),
            compute_p2id_tag_for_local_account(bob.id()),
        )?;

        let mut advice_felts: Vec<Felt> = Vec::new();
        let mut expected_spread_count = 0usize;
        if eth_spread > 0 {
            let w = Word::from(Asset::from(FungibleAsset::new(
                eth_faucet.id(),
                eth_spread,
            )?));
            advice_felts.extend(w);
            expected_spread_count += 1;
        }
        if usdc_spread > 0 {
            let w = Word::from(Asset::from(FungibleAsset::new(
                usdc_faucet.id(),
                usdc_spread,
            )?));
            advice_felts.extend(w);
            expected_spread_count += 1;
        }

        let commitment_key: Word = Rpo256::hash_elements(&advice_felts);
        let mut commitment = commitment_key;
        commitment.reverse();

        let tx_context = mock_chain
            .build_tx_context(charlie.id(), &[alice_note.id(), bob_note.id()], &[])?
            .tx_script(tx_script)
            .tx_script_args(commitment)
            .extend_advice_map([(commitment_key, advice_felts)])
            .extend_expected_output_notes(vec![
                OutputNote::Full(alice_p2id),
                OutputNote::Full(bob_p2id),
            ])
            .extend_note_args(note_args_map)
            .build()?;

        let executed_tx = tx_context.execute().await?;

        let output_notes = executed_tx.output_notes();
        assert_eq!(
            output_notes.num_notes(),
            2,
            "Case {}: Expected 2 notes",
            i + 1
        );

        let vault_delta = executed_tx.account_delta().vault();
        assert_eq!(vault_delta.removed_assets().count(), 0, "Case {}", i + 1);
        let added: Vec<Asset> = vault_delta.added_assets().collect();
        assert_eq!(added.len(), expected_spread_count, "Case {}", i + 1);

        for asset in &added {
            if let Asset::Fungible(f) = asset {
                if f.faucet_id() == eth_faucet.id() {
                    assert_eq!(f.amount(), eth_spread, "Case {}: ETH spread", i + 1);
                } else if f.faucet_id() == usdc_faucet.id() {
                    assert_eq!(f.amount(), usdc_spread, "Case {}: USDC spread", i + 1);
                }
            }
        }

        println!("  PASSED: {}ETH+{}USDC spread", eth_spread, usdc_spread);
    }

    println!("\n=== All {} ultimate fuzz test cases passed! ===", total);
    Ok(())
}

#[tokio::test]
async fn pswap_note_partial_fill_non_integer_ratio_fuzz_test() -> anyhow::Result<()> {
    println!("=== Fuzz Test: PSWAP Partial Fill With Non-Integer Ratios ===\n");

    let pswap_script = compile_pswap_note_script();

    // (offered_usdc, requested_eth, fill_eth)
    let mut test_cases: Vec<(u64, u64, u64)> = vec![
        (23, 20, 7),
        (23, 20, 13),
        (23, 20, 19),
        (17, 13, 5),
        (97, 89, 37),
        (53, 47, 23),
        (7, 5, 3),
        (7, 5, 1),
        (7, 5, 4),
        (89, 55, 21),
        (233, 144, 55),
        (34, 21, 8),
        (50, 97, 30),
        (13, 47, 20),
        (3, 7, 5),
        (101, 100, 50),
        (100, 99, 50),
        (997, 991, 500),
        (1000, 3, 1),
        (1000, 3, 2),
        (3, 1000, 500),
        (9999, 7777, 3333),
        (5000, 3333, 1111),
        (127, 63, 31),
        (255, 127, 63),
        (511, 255, 100),
    ];

    use rand::Rng;
    let mut thread_rng = rand::rng();
    let mut random_added = 0;
    while random_added < 20 {
        let offered: u64 = thread_rng.random_range(3..2000);
        let requested: u64 = thread_rng.random_range(2..2000);
        if offered % requested == 0 || requested % offered == 0 {
            continue;
        }
        let fill: u64 = thread_rng.random_range(1..requested);
        let out = calculate_output_amount(offered, requested, fill);
        if out == 0 {
            continue;
        }
        test_cases.push((offered, requested, fill));
        random_added += 1;
    }

    let total = test_cases.len();
    println!("Running {} test cases...\n", total);

    for (i, (offered_usdc, requested_eth, fill_eth)) in test_cases.iter().enumerate() {
        let offered_out = calculate_output_amount(*offered_usdc, *requested_eth, *fill_eth);
        let remaining_offered = offered_usdc - offered_out;
        let remaining_requested = requested_eth - fill_eth;

        println!(
            "--- Case {}/{}: {}USDC/{}ETH fill={} -> out={} rem={}USDC/{}ETH ---",
            i + 1,
            total,
            offered_usdc,
            requested_eth,
            fill_eth,
            offered_out,
            remaining_offered,
            remaining_requested,
        );

        assert!(offered_out > 0, "Case {}: offered_out must be > 0", i + 1);
        assert!(
            offered_out <= *offered_usdc,
            "Case {}: offered_out > offered",
            i + 1
        );

        let mut builder = MockChain::builder();
        let max_amount = *[*offered_usdc, *requested_eth].iter().max().unwrap();
        let total_issuance = max_amount * 10 + 1000;

        let usdc_faucet = builder.add_existing_basic_faucet(
            Auth::BasicAuth,
            "USDC",
            10000,
            Some(total_issuance),
        )?;
        let eth_faucet = builder.add_existing_basic_faucet(
            Auth::BasicAuth,
            "ETH",
            10000,
            Some(total_issuance),
        )?;

        let alice = builder.add_existing_wallet_with_assets(
            Auth::BasicAuth,
            [FungibleAsset::new(usdc_faucet.id(), *offered_usdc)?.into()],
        )?;

        let bob = builder.add_existing_wallet_with_assets(
            Auth::BasicAuth,
            [FungibleAsset::new(eth_faucet.id(), *fill_eth)?.into()],
        )?;

        let swapp_tag = NoteTag::new(0xC0000000);
        let swapp_tag_felt = Felt::new(u32::from(swapp_tag) as u64);
        let p2id_tag_felt = compute_p2id_tag_felt(alice.id());

        let note_inputs = build_pswap_inputs(
            eth_faucet.id(),
            *requested_eth,
            swapp_tag_felt,
            p2id_tag_felt,
            0,
            alice.id(),
        );
        let mut note_assets = NoteAssets::default();
        note_assets.add_asset(FungibleAsset::new(usdc_faucet.id(), *offered_usdc)?.into())?;
        let swap_note = create_pswap_note(
            pswap_script.clone(),
            alice.id(),
            note_assets,
            note_inputs,
            swapp_tag,
        );
        builder.add_output_note(OutputNote::Full(swap_note.clone()));

        let mock_chain = builder.build()?;

        let note_args = Word::from([Felt::ZERO, Felt::ZERO, Felt::ZERO, Felt::new(*fill_eth)]);
        let mut note_args_map = BTreeMap::new();
        note_args_map.insert(swap_note.id(), note_args);

        let p2id_note = create_expected_pswap_p2id_note(
            &swap_note,
            bob.id(),
            alice.id(),
            0,
            *fill_eth,
            eth_faucet.id(),
            compute_p2id_tag_for_local_account(alice.id()),
        )?;

        let mut expected_notes = vec![OutputNote::Full(p2id_note)];
        if remaining_requested > 0 {
            let remainder = create_expected_pswap_remainder_note(
                &swap_note,
                pswap_script.clone(),
                bob.id(),
                alice.id(),
                remaining_offered,
                remaining_requested,
                offered_out,
                0,
                usdc_faucet.id(),
                eth_faucet.id(),
                swapp_tag,
                swapp_tag_felt,
                p2id_tag_felt,
            )?;
            expected_notes.push(OutputNote::Full(remainder));
        }

        let tx_context = mock_chain
            .build_tx_context(bob.id(), &[swap_note.id()], &[])?
            .extend_expected_output_notes(expected_notes)
            .extend_note_args(note_args_map)
            .build()?;

        let executed_tx = tx_context.execute().await.map_err(|e| {
            anyhow::anyhow!(
                "Case {} failed: {} (offered={}, requested={}, fill={})",
                i + 1,
                e,
                offered_usdc,
                requested_eth,
                fill_eth
            )
        })?;

        let output_notes = executed_tx.output_notes();
        let expected_count = if remaining_requested > 0 { 2 } else { 1 };
        assert_eq!(output_notes.num_notes(), expected_count, "Case {}", i + 1);

        let vault_delta = executed_tx.account_delta().vault();
        let added: Vec<Asset> = vault_delta.added_assets().collect();
        let removed: Vec<Asset> = vault_delta.removed_assets().collect();
        assert_eq!(added.len(), 1, "Case {}", i + 1);
        if let Asset::Fungible(f) = &added[0] {
            assert_eq!(f.amount(), offered_out, "Case {}", i + 1);
        }
        assert_eq!(removed.len(), 1, "Case {}", i + 1);
        if let Asset::Fungible(f) = &removed[0] {
            assert_eq!(f.amount(), *fill_eth, "Case {}", i + 1);
        }

        assert_eq!(
            offered_out + remaining_offered,
            *offered_usdc,
            "Case {}: conservation",
            i + 1
        );
        println!(
            "  PASSED: Bob got {} USDC for {} ETH",
            offered_out, fill_eth
        );
    }

    println!(
        "\n=== All {} PSWAP partial fill non-integer ratio cases passed! ===",
        total
    );
    Ok(())
}

/// Test that PSWAP.masm handles non-exact ratios correctly (unlike swapp-note which uses Felt division).
/// PSWAP uses u64 integer arithmetic, so 100/30 should truncate correctly.
#[tokio::test]
async fn pswap_note_non_exact_ratio_partial_fill_test() -> anyhow::Result<()> {
    println!("=== Test: PSWAP Non-Exact Ratio Partial Fill (u64 arithmetic) ===\n");

    let offered_total = 100u64;
    let requested_total = 30u64;
    let input_amount = 7u64;
    let expected_output = calculate_output_amount(offered_total, requested_total, input_amount);
    println!(
        "100 USDC / 30 ETH, fill=7 ETH -> expected output={} USDC",
        expected_output
    );

    let mut builder = MockChain::builder();
    let usdc_faucet =
        builder.add_existing_basic_faucet(Auth::BasicAuth, "USDC", 10000, Some(1000))?;
    let eth_faucet = builder.add_existing_basic_faucet(Auth::BasicAuth, "ETH", 10000, Some(100))?;

    let alice = builder.add_existing_wallet_with_assets(
        Auth::BasicAuth,
        [FungibleAsset::new(usdc_faucet.id(), offered_total)?.into()],
    )?;

    let bob = builder.add_existing_wallet_with_assets(
        Auth::BasicAuth,
        [FungibleAsset::new(eth_faucet.id(), input_amount)?.into()],
    )?;

    let pswap_script = compile_pswap_note_script();
    let swapp_tag = NoteTag::new(0xC0000000);
    let swapp_tag_felt = Felt::new(u32::from(swapp_tag) as u64);
    let p2id_tag_felt = compute_p2id_tag_felt(alice.id());

    let note_inputs = build_pswap_inputs(
        eth_faucet.id(),
        requested_total,
        swapp_tag_felt,
        p2id_tag_felt,
        0,
        alice.id(),
    );
    let mut note_assets = NoteAssets::default();
    note_assets.add_asset(FungibleAsset::new(usdc_faucet.id(), offered_total)?.into())?;
    let swap_note = create_pswap_note(
        pswap_script.clone(),
        alice.id(),
        note_assets,
        note_inputs,
        swapp_tag,
    );
    builder.add_output_note(OutputNote::Full(swap_note.clone()));

    let mock_chain = builder.build()?;

    let note_args = Word::from([Felt::ZERO, Felt::ZERO, Felt::ZERO, Felt::new(input_amount)]);
    let mut note_args_map = BTreeMap::new();
    note_args_map.insert(swap_note.id(), note_args);

    let remaining_offered = offered_total - expected_output;
    let remaining_requested = requested_total - input_amount;

    let p2id_note = create_expected_pswap_p2id_note(
        &swap_note,
        bob.id(),
        alice.id(),
        0,
        input_amount,
        eth_faucet.id(),
        compute_p2id_tag_for_local_account(alice.id()),
    )?;
    let remainder = create_expected_pswap_remainder_note(
        &swap_note,
        pswap_script,
        bob.id(),
        alice.id(),
        remaining_offered,
        remaining_requested,
        expected_output,
        0,
        usdc_faucet.id(),
        eth_faucet.id(),
        swapp_tag,
        swapp_tag_felt,
        p2id_tag_felt,
    )?;

    let tx_context = mock_chain
        .build_tx_context(bob.id(), &[swap_note.id()], &[])?
        .extend_expected_output_notes(vec![
            OutputNote::Full(p2id_note),
            OutputNote::Full(remainder),
        ])
        .extend_note_args(note_args_map)
        .build()?;

    // PSWAP uses u64 arithmetic, so this should SUCCEED (unlike swapp-note which fails)
    let executed_tx = tx_context.execute().await?;

    let output_notes = executed_tx.output_notes();
    assert_eq!(output_notes.num_notes(), 2);

    let vault_delta = executed_tx.account_delta().vault();
    let added: Vec<Asset> = vault_delta.added_assets().collect();
    assert_eq!(added.len(), 1);
    if let Asset::Fungible(f) = &added[0] {
        assert_eq!(f.amount(), expected_output);
        println!("Bob received {} USDC (correct u64 truncation)", f.amount());
    }

    println!("\nPSWAP non-exact ratio test passed! u64 arithmetic works correctly.");
    Ok(())
}

/// Fuzz test: inflight cross-swap where BOTH notes are partially filled.
/// Both notes produce P2ID + remainder notes, plus Bob earns a spread.
#[tokio::test]
async fn pswap_note_inflight_partial_fill_cross_swap_fuzz_test() -> anyhow::Result<()> {
    println!("=== Fuzz Test: PSWAP Inflight Partial Fill Cross Swap ===\n");

    let consume_asset_package = Arc::new(build_project_in_dir(
        Path::new("../contracts/consume-asset-script"),
        true,
    )?);
    let pswap_script = compile_pswap_note_script();

    // (alice_eth, alice_usdc, bob_usdc, bob_eth, bob_fill_eth)
    let test_cases: Vec<(u64, u64, u64, u64, u64)> = vec![
        (100, 50, 60, 30, 15),
        (10, 5, 8, 4, 2),
        (50, 25, 30, 15, 5),
        (20, 10, 14, 7, 3),
        (6, 3, 4, 2, 1),
        (40, 20, 10, 5, 2),
        (14, 7, 10, 5, 3),
        (90, 45, 80, 40, 20),
        (30, 15, 24, 8, 4),
        (200, 100, 150, 50, 25),
        (10, 20, 30, 10, 4),
    ];

    let total = test_cases.len();
    println!("Running {} test cases...\n", total);

    for (i, (alice_eth, alice_usdc, bob_usdc, bob_eth, bob_fill_eth)) in
        test_cases.iter().enumerate()
    {
        let bob_offered_out = calculate_output_amount(*bob_usdc, *bob_eth, *bob_fill_eth);
        let alice_fill_usdc = bob_offered_out;
        let alice_offered_out = calculate_output_amount(*alice_eth, *alice_usdc, alice_fill_usdc);

        assert!(
            *bob_fill_eth < *bob_eth,
            "Case {}: Bob fill must be partial",
            i + 1
        );
        assert!(
            alice_fill_usdc < *alice_usdc,
            "Case {}: Alice fill must be partial",
            i + 1
        );
        assert!(
            alice_offered_out > *bob_fill_eth,
            "Case {}: Need positive spread",
            i + 1
        );

        let spread = alice_offered_out - bob_fill_eth;
        let alice_remaining_eth = alice_eth - alice_offered_out;
        let bob_remaining_usdc = bob_usdc - bob_offered_out;
        let alice_remaining_usdc = alice_usdc - alice_fill_usdc;
        let bob_remaining_eth = bob_eth - bob_fill_eth;

        println!(
            "--- Case {}/{}: Alice {}ETH for {}USDC | Bob {}USDC for {}ETH | fill={}ETH spread={}ETH ---",
            i + 1, total, alice_eth, alice_usdc, bob_usdc, bob_eth, bob_fill_eth, spread
        );

        let mut builder = MockChain::builder();
        let max_amount = *[*alice_eth, *alice_usdc, *bob_usdc, *bob_eth]
            .iter()
            .max()
            .unwrap();
        let total_issuance = max_amount * 10;

        let usdc_faucet = builder.add_existing_basic_faucet(
            Auth::BasicAuth,
            "USDC",
            1000,
            Some(total_issuance),
        )?;
        let eth_faucet = builder.add_existing_basic_faucet(
            Auth::BasicAuth,
            "ETH",
            1000,
            Some(total_issuance),
        )?;

        let alice = builder.add_existing_wallet_with_assets(
            Auth::BasicAuth,
            [FungibleAsset::new(eth_faucet.id(), *alice_eth)?.into()],
        )?;
        let bob = builder.add_existing_wallet_with_assets(
            Auth::BasicAuth,
            [FungibleAsset::new(usdc_faucet.id(), *bob_usdc)?.into()],
        )?;

        let charlie = builder.add_existing_wallet_with_assets(Auth::BasicAuth, [])?;

        let swapp_tag = NoteTag::new(0xC0000000);
        let swapp_tag_felt = Felt::new(u32::from(swapp_tag) as u64);
        let alice_p2id_felt = compute_p2id_tag_felt(alice.id());
        let bob_p2id_felt = compute_p2id_tag_felt(bob.id());

        // Alice's note: offers alice_eth ETH, requests alice_usdc USDC
        let alice_inputs = build_pswap_inputs(
            usdc_faucet.id(),
            *alice_usdc,
            swapp_tag_felt,
            alice_p2id_felt,
            0,
            alice.id(),
        );
        let mut alice_assets = NoteAssets::default();
        alice_assets.add_asset(FungibleAsset::new(eth_faucet.id(), *alice_eth)?.into())?;
        let alice_note = create_pswap_note(
            pswap_script.clone(),
            alice.id(),
            alice_assets,
            alice_inputs,
            swapp_tag,
        );
        builder.add_output_note(OutputNote::Full(alice_note.clone()));

        // Bob's note: offers bob_usdc USDC, requests bob_eth ETH
        let bob_inputs = build_pswap_inputs(
            eth_faucet.id(),
            *bob_eth,
            swapp_tag_felt,
            bob_p2id_felt,
            0,
            bob.id(),
        );
        let mut bob_assets = NoteAssets::default();
        bob_assets.add_asset(FungibleAsset::new(usdc_faucet.id(), *bob_usdc)?.into())?;
        let bob_note = create_pswap_note(
            pswap_script.clone(),
            bob.id(),
            bob_assets,
            bob_inputs,
            swapp_tag,
        );
        builder.add_output_note(OutputNote::Full(bob_note.clone()));

        let program = consume_asset_package.unwrap_program();
        let tx_script =
            TransactionScript::from_parts(program.mast_forest().clone(), program.entrypoint());

        let mock_chain = builder.build()?;

        // Note args: both inflight partial fills
        let alice_args = Word::from([
            Felt::ZERO,
            Felt::ZERO,
            Felt::new(alice_fill_usdc),
            Felt::ZERO,
        ]);
        let bob_args = Word::from([Felt::ZERO, Felt::ZERO, Felt::new(*bob_fill_eth), Felt::ZERO]);
        let mut note_args_map = BTreeMap::new();
        note_args_map.insert(alice_note.id(), alice_args);
        note_args_map.insert(bob_note.id(), bob_args);

        // Expected output notes
        let alice_p2id = create_expected_pswap_p2id_note(
            &alice_note,
            charlie.id(),
            alice.id(),
            0,
            alice_fill_usdc,
            usdc_faucet.id(),
            compute_p2id_tag_for_local_account(alice.id()),
        )?;
        let alice_remainder = create_expected_pswap_remainder_note(
            &alice_note,
            pswap_script.clone(),
            charlie.id(),
            alice.id(),
            alice_remaining_eth,
            alice_remaining_usdc,
            alice_offered_out,
            0,
            eth_faucet.id(),
            usdc_faucet.id(),
            swapp_tag,
            swapp_tag_felt,
            alice_p2id_felt,
        )?;
        let bob_p2id = create_expected_pswap_p2id_note(
            &bob_note,
            charlie.id(),
            bob.id(),
            0,
            *bob_fill_eth,
            eth_faucet.id(),
            compute_p2id_tag_for_local_account(bob.id()),
        )?;
        let bob_remainder = create_expected_pswap_remainder_note(
            &bob_note,
            pswap_script.clone(),
            charlie.id(),
            bob.id(),
            bob_remaining_usdc,
            bob_remaining_eth,
            bob_offered_out,
            0,
            usdc_faucet.id(),
            eth_faucet.id(),
            swapp_tag,
            swapp_tag_felt,
            bob_p2id_felt,
        )?;

        // Advice stack for spread
        let spread_word = Word::from(Asset::from(FungibleAsset::new(eth_faucet.id(), spread)?));
        let advice_stack: Vec<Felt> = spread_word.to_vec();
        let commitment_key: Word = Rpo256::hash_elements(&advice_stack);
        let mut commitment = commitment_key;
        commitment.reverse();

        let tx_context = mock_chain
            .build_tx_context(charlie.id(), &[alice_note.id(), bob_note.id()], &[])?
            .tx_script(tx_script)
            .tx_script_args(commitment)
            .extend_advice_map([(commitment_key, advice_stack)])
            .extend_expected_output_notes(vec![
                OutputNote::Full(alice_p2id),
                OutputNote::Full(alice_remainder),
                OutputNote::Full(bob_p2id),
                OutputNote::Full(bob_remainder),
            ])
            .extend_note_args(note_args_map)
            .build()?;

        let executed_tx = tx_context.execute().await?;

        let output_notes = executed_tx.output_notes();
        assert_eq!(
            output_notes.num_notes(),
            4,
            "Case {}: Expected 4 notes",
            i + 1
        );

        // Verify Charlie's vault: spread only
        let vault_delta = executed_tx.account_delta().vault();
        assert_eq!(vault_delta.removed_assets().count(), 0, "Case {}", i + 1);
        let added: Vec<Asset> = vault_delta.added_assets().collect();
        assert_eq!(added.len(), 1, "Case {}: 1 spread asset", i + 1);
        if let Asset::Fungible(f) = &added[0] {
            assert_eq!(f.faucet_id(), eth_faucet.id(), "Case {}", i + 1);
            assert_eq!(f.amount(), spread, "Case {}: spread={}", i + 1, spread);
        }

        println!(
            "  PASSED: 4 notes | Alice P2ID {}USDC + Rem {}ETH | Bob P2ID {}ETH + Rem {}USDC | Charlie {}ETH spread",
            alice_fill_usdc, alice_remaining_eth, bob_fill_eth, bob_remaining_usdc, spread
        );
    }

    println!(
        "\n=== All {} PSWAP partial fill fuzz test cases passed! ===",
        total
    );
    Ok(())
}

/// Test: Chained partial fills on the same swap note with non-integer ratios.
/// Each fill consumes the remainder from the previous fill.
#[tokio::test]
async fn pswap_note_chained_partial_fills_non_integer_ratio_test() -> anyhow::Result<()> {
    println!("=== Test: PSWAP Chained Partial Fills With Non-Integer Ratios ===\n");

    let pswap_script = compile_pswap_note_script();

    let test_chains: Vec<(u64, u64, Vec<u64>, &str)> = vec![
        (100, 73, vec![17, 23, 19], "100/73 ratio, 3 fills"),
        (53, 47, vec![7, 11, 13, 5], "53/47 ratio, 4 fills"),
        (200, 137, vec![41, 37, 29], "200/137 ratio, 3 fills"),
        (7, 5, vec![2, 1], "7/5 ratio, 2 fills"),
        (
            1000,
            777,
            vec![100, 200, 150, 100],
            "1000/777 ratio, 4 fills",
        ),
        (
            50,
            97,
            vec![20, 30, 15],
            "50/97 ratio (offered<req), 3 fills",
        ),
        (89, 55, vec![13, 8, 21], "89/55 Fibonacci ratio, 3 fills"),
        (
            23,
            20,
            vec![3, 5, 4, 3],
            "23/20 ratio (original bug), 4 fills",
        ),
        (
            997,
            991,
            vec![300, 300, 200],
            "997/991 large primes near 1:1, 3 fills",
        ),
        (3, 2, vec![1], "3/2 ratio, 1 fill"),
    ];

    for (chain_idx, (initial_offered, initial_requested, fills, description)) in
        test_chains.iter().enumerate()
    {
        println!(
            "\nChain {}/{}: {} USDC for {} ETH — {}",
            chain_idx + 1,
            test_chains.len(),
            initial_offered,
            initial_requested,
            description
        );

        let mut current_offered = *initial_offered;
        let mut current_requested = *initial_requested;
        let mut total_usdc_to_bob = 0u64;
        let mut total_eth_from_bob = 0u64;
        let mut current_swap_count = 0u64;

        // Track serial for remainder chain
        use miden_crypto::rand::{FeltRng, RpoRandomCoin};
        let mut rng = RpoRandomCoin::new(Word::default());
        let mut current_serial = rng.draw_word();

        for (fill_idx, fill_amount) in fills.iter().enumerate() {
            let offered_out =
                calculate_output_amount(current_offered, current_requested, *fill_amount);
            let remaining_offered = current_offered - offered_out;
            let remaining_requested = current_requested - fill_amount;

            println!(
                "  Fill {}: Bob provides {} ETH -> gets {} USDC | remainder: {} USDC for {} ETH",
                fill_idx + 1,
                fill_amount,
                offered_out,
                remaining_offered,
                remaining_requested
            );

            let mut builder = MockChain::builder();
            let max_amount = *[current_offered, current_requested, *fill_amount]
                .iter()
                .max()
                .unwrap();
            let total_issuance = max_amount * 10 + 1000;

            let usdc_faucet = builder.add_existing_basic_faucet(
                Auth::BasicAuth,
                "USDC",
                10000,
                Some(total_issuance),
            )?;
            let eth_faucet = builder.add_existing_basic_faucet(
                Auth::BasicAuth,
                "ETH",
                10000,
                Some(total_issuance),
            )?;

            let alice = builder.add_existing_wallet_with_assets(
                Auth::BasicAuth,
                [FungibleAsset::new(usdc_faucet.id(), current_offered)?.into()],
            )?;

            let bob = builder.add_existing_wallet_with_assets(
                Auth::BasicAuth,
                [FungibleAsset::new(eth_faucet.id(), *fill_amount)?.into()],
            )?;

            let swapp_tag = NoteTag::new(0xC0000000);
            let swapp_tag_felt = Felt::new(u32::from(swapp_tag) as u64);
            let p2id_tag_felt = compute_p2id_tag_felt(alice.id());

            let note_inputs = build_pswap_inputs(
                eth_faucet.id(),
                current_requested,
                swapp_tag_felt,
                p2id_tag_felt,
                current_swap_count,
                alice.id(),
            );
            let mut note_assets = NoteAssets::default();
            note_assets.add_asset(FungibleAsset::new(usdc_faucet.id(), current_offered)?.into())?;

            // Create note with the correct serial for this chain position
            let note_inputs_obj = NoteInputs::new(note_inputs)?;
            let recipient =
                NoteRecipient::new(current_serial, pswap_script.clone(), note_inputs_obj);
            let metadata = NoteMetadata::new(alice.id(), NoteType::Public, swapp_tag);
            let swap_note = Note::new(note_assets, metadata, recipient);

            builder.add_output_note(OutputNote::Full(swap_note.clone()));
            let mock_chain = builder.build()?;

            let note_args =
                Word::from([Felt::ZERO, Felt::ZERO, Felt::ZERO, Felt::new(*fill_amount)]);
            let mut note_args_map = BTreeMap::new();
            note_args_map.insert(swap_note.id(), note_args);

            let p2id_note = create_expected_pswap_p2id_note(
                &swap_note,
                bob.id(),
                alice.id(),
                current_swap_count,
                *fill_amount,
                eth_faucet.id(),
                compute_p2id_tag_for_local_account(alice.id()),
            )?;

            let mut expected_notes = vec![OutputNote::Full(p2id_note)];
            if remaining_requested > 0 {
                let remainder = create_expected_pswap_remainder_note(
                    &swap_note,
                    pswap_script.clone(),
                    bob.id(),
                    alice.id(),
                    remaining_offered,
                    remaining_requested,
                    offered_out,
                    current_swap_count,
                    usdc_faucet.id(),
                    eth_faucet.id(),
                    swapp_tag,
                    swapp_tag_felt,
                    p2id_tag_felt,
                )?;
                expected_notes.push(OutputNote::Full(remainder));
            }

            let tx_context = mock_chain
                .build_tx_context(bob.id(), &[swap_note.id()], &[])?
                .extend_expected_output_notes(expected_notes)
                .extend_note_args(note_args_map)
                .build()?;

            let executed_tx = tx_context.execute().await.map_err(|e| {
                anyhow::anyhow!(
                    "Chain {} fill {} failed: {} (offered={}, requested={}, fill={})",
                    chain_idx + 1,
                    fill_idx + 1,
                    e,
                    current_offered,
                    current_requested,
                    fill_amount
                )
            })?;

            let output_notes = executed_tx.output_notes();
            let expected_count = if remaining_requested > 0 { 2 } else { 1 };
            assert_eq!(
                output_notes.num_notes(),
                expected_count,
                "Chain {} fill {}",
                chain_idx + 1,
                fill_idx + 1
            );

            let vault_delta = executed_tx.account_delta().vault();
            let added: Vec<Asset> = vault_delta.added_assets().collect();
            assert_eq!(
                added.len(),
                1,
                "Chain {} fill {}",
                chain_idx + 1,
                fill_idx + 1
            );
            if let Asset::Fungible(f) = &added[0] {
                assert_eq!(
                    f.amount(),
                    offered_out,
                    "Chain {} fill {}: Bob should get {} USDC",
                    chain_idx + 1,
                    fill_idx + 1,
                    offered_out
                );
            }

            // Update state for next fill
            total_usdc_to_bob += offered_out;
            total_eth_from_bob += fill_amount;
            current_offered = remaining_offered;
            current_requested = remaining_requested;
            current_swap_count += 1;
            // Remainder serial: [3] + 1
            current_serial = Word::from([
                current_serial[0],
                current_serial[1],
                current_serial[2],
                Felt::new(current_serial[3].as_int() + 1),
            ]);
        }

        // Verify conservation
        let total_fills: u64 = fills.iter().sum();
        assert_eq!(
            total_eth_from_bob,
            total_fills,
            "Chain {}: ETH conservation",
            chain_idx + 1
        );
        assert_eq!(
            total_usdc_to_bob + current_offered,
            *initial_offered,
            "Chain {}: USDC conservation",
            chain_idx + 1
        );

        println!(
            "  PASSED: {} fills, Bob got {} USDC for {} ETH, remainder: {} USDC for {} ETH",
            fills.len(),
            total_usdc_to_bob,
            total_eth_from_bob,
            current_offered,
            current_requested
        );
    }

    println!("\n=== All PSWAP chained partial fill test chains passed! ===");
    Ok(())
}

#[test]
fn check_p2id_script_root_matches_pswap_masm() {
    use miden_standards::note::WellKnownNote;
    let p2id_script = WellKnownNote::P2ID.script();
    let root = p2id_script.root();
    // Values from PSWAP.masm line 220:
    // push.13362761878458161062.15090726097241769395.444910447169617901.3558201871398422326
    let masm_values: [u64; 4] = [
        13362761878458161062,
        15090726097241769395,
        444910447169617901,
        3558201871398422326,
    ];
    for (i, f) in root.iter().enumerate() {
        println!("Rust P2ID root[{}] = {}", i, f.as_int());
        println!("MASM P2ID root[{}] = {}", i, masm_values[i]);
    }
    for (i, f) in root.iter().enumerate() {
        assert_eq!(
            f.as_int(),
            masm_values[i],
            "P2ID script root mismatch at [{}]!",
            i
        );
    }
    println!("P2ID script root MATCHES between Rust and PSWAP.masm");
}

#[test]
fn compare_pswap_create_output_notes_vs_test_helper() {
    use miden_crypto::rand::{FeltRng, RpoRandomCoin};
    use miden_swapp::PswapNote;

    let mut builder = MockChain::builder();
    let usdc_faucet = builder
        .add_existing_basic_faucet(Auth::BasicAuth, "USDC", 1000, Some(150))
        .unwrap();
    let eth_faucet = builder
        .add_existing_basic_faucet(Auth::BasicAuth, "ETH", 1000, Some(50))
        .unwrap();
    let alice = builder
        .add_existing_wallet_with_assets(
            Auth::BasicAuth,
            [FungibleAsset::new(usdc_faucet.id(), 50).unwrap().into()],
        )
        .unwrap();
    let bob = builder
        .add_existing_wallet_with_assets(
            Auth::BasicAuth,
            [FungibleAsset::new(eth_faucet.id(), 25).unwrap().into()],
        )
        .unwrap();

    // Create swap note using PswapNote::create (same as bin file)
    let mut rng = RpoRandomCoin::new(Word::default());
    let swap_note_lib = PswapNote::create(
        alice.id(),
        Asset::Fungible(FungibleAsset::new(usdc_faucet.id(), 50).unwrap()),
        Asset::Fungible(FungibleAsset::new(eth_faucet.id(), 25).unwrap()),
        NoteType::Public,
        miden_protocol::note::NoteAttachment::default(),
        &mut rng,
    )
    .unwrap();

    // Create output notes using library
    let (lib_p2id, _) = PswapNote::create_output_notes(&swap_note_lib, bob.id(), 25, 0).unwrap();

    // Create same swap note using test helper (same as pswap_test.rs)
    let pswap_script = compile_pswap_note_script();
    let (swapp_tag, swapp_tag_felt) = make_swapp_tag();
    let p2id_tag_felt = compute_p2id_tag_felt(alice.id());
    let note_inputs = build_pswap_inputs(
        eth_faucet.id(),
        25,
        swapp_tag_felt,
        p2id_tag_felt,
        0,
        alice.id(),
    );
    let note_assets = make_note_assets(usdc_faucet.id(), 50).unwrap();

    // Use the SAME serial as the library note
    let test_serial = swap_note_lib.recipient().serial_num();
    let test_note_inputs = NoteInputs::new(note_inputs).unwrap();
    let test_recipient = NoteRecipient::new(test_serial, pswap_script.clone(), test_note_inputs);
    let test_metadata = NoteMetadata::new(alice.id(), NoteType::Public, swapp_tag);
    let swap_note_test = Note::new(note_assets, test_metadata, test_recipient);

    // Create expected P2ID using test helper
    let test_p2id = create_expected_pswap_p2id_note(
        &swap_note_test,
        bob.id(),
        alice.id(),
        0,
        25,
        eth_faucet.id(),
        compute_p2id_tag_for_local_account(alice.id()),
    )
    .unwrap();

    // Compare
    println!("=== Library P2ID ===");
    println!("  ID: {:?}", lib_p2id.id());
    println!("  Recipient digest: {:?}", lib_p2id.recipient().digest());
    println!("  Serial: {:?}", lib_p2id.recipient().serial_num());
    println!("  Script root: {:?}", lib_p2id.recipient().script().root());
    println!("  Tag: {:?}", lib_p2id.metadata().tag());
    println!("  Sender: {:?}", lib_p2id.metadata().sender());
    println!("  Note type: {:?}", lib_p2id.metadata().note_type());
    println!(
        "  Assets: {:?}",
        lib_p2id.assets().iter().collect::<Vec<_>>()
    );

    println!("\n=== Test P2ID ===");
    println!("  ID: {:?}", test_p2id.id());
    println!("  Recipient digest: {:?}", test_p2id.recipient().digest());
    println!("  Serial: {:?}", test_p2id.recipient().serial_num());
    println!("  Script root: {:?}", test_p2id.recipient().script().root());
    println!("  Tag: {:?}", test_p2id.metadata().tag());
    println!("  Sender: {:?}", test_p2id.metadata().sender());
    println!("  Note type: {:?}", test_p2id.metadata().note_type());
    println!(
        "  Assets: {:?}",
        test_p2id.assets().iter().collect::<Vec<_>>()
    );

    // Check each component
    assert_eq!(
        lib_p2id.recipient().serial_num(),
        test_p2id.recipient().serial_num(),
        "Serial mismatch!"
    );
    assert_eq!(
        lib_p2id.recipient().script().root(),
        test_p2id.recipient().script().root(),
        "Script root mismatch!"
    );
    assert_eq!(
        lib_p2id.recipient().digest(),
        test_p2id.recipient().digest(),
        "Recipient digest mismatch!"
    );
    assert_eq!(
        lib_p2id.metadata().tag(),
        test_p2id.metadata().tag(),
        "Tag mismatch!"
    );
    assert_eq!(
        lib_p2id.metadata().sender(),
        test_p2id.metadata().sender(),
        "Sender mismatch!"
    );
    assert_eq!(
        lib_p2id.metadata().note_type(),
        test_p2id.metadata().note_type(),
        "Note type mismatch!"
    );
    assert_eq!(lib_p2id.id(), test_p2id.id(), "NOTE ID MISMATCH!");
    println!("\nAll components match!");
}

#[test]
fn debug_parse_inputs_roundtrip() {
    use miden_crypto::rand::{FeltRng, RpoRandomCoin};
    use miden_swapp::PswapNote;

    let mut builder = MockChain::builder();
    let usdc_faucet = builder
        .add_existing_basic_faucet(Auth::BasicAuth, "USDC", 1000, Some(150))
        .unwrap();
    let eth_faucet = builder
        .add_existing_basic_faucet(Auth::BasicAuth, "ETH", 1000, Some(50))
        .unwrap();
    let alice = builder
        .add_existing_wallet_with_assets(
            Auth::BasicAuth,
            [FungibleAsset::new(usdc_faucet.id(), 50).unwrap().into()],
        )
        .unwrap();

    let mut rng = RpoRandomCoin::new(Word::default());
    let swap_note = PswapNote::create(
        alice.id(),
        Asset::Fungible(FungibleAsset::new(usdc_faucet.id(), 50).unwrap()),
        Asset::Fungible(FungibleAsset::new(eth_faucet.id(), 25).unwrap()),
        NoteType::Public,
        miden_protocol::note::NoteAttachment::default(),
        &mut rng,
    )
    .unwrap();

    let inputs = swap_note.recipient().inputs();
    let values = inputs.values();

    println!("=== Raw input values ({} elements) ===", values.len());
    for (i, v) in values.iter().enumerate() {
        println!("  inputs[{}] = {}", i, v.as_int());
    }

    println!("\n=== Expected layout ===");
    println!(
        "  [0] requested faucet prefix: {}",
        eth_faucet.id().prefix().as_felt().as_int()
    );
    println!(
        "  [1] requested faucet suffix: {}",
        eth_faucet.id().suffix().as_int()
    );
    println!("  [2] padding: 0");
    println!("  [3] requested amount: 25");
    println!("  [4] swapp_tag: (computed)");
    println!("  [5] p2id_tag: (computed)");
    println!("  [6-7] reserved: 0");
    println!("  [8] swap_count: 0");
    println!("  [9-11] reserved: 0");
    println!(
        "  [12] creator prefix: {}",
        alice.id().prefix().as_felt().as_int()
    );
    println!("  [13] creator suffix: {}", alice.id().suffix().as_int());

    // Verify parse_inputs roundtrip
    let (requested_word, swapp_tag, p2id_tag, swap_count, creator_id) =
        PswapNote::parse_inputs(values).unwrap();

    println!("\n=== Parsed values ===");
    println!(
        "  requested_word: {:?}",
        requested_word
            .iter()
            .map(|f| f.as_int())
            .collect::<Vec<_>>()
    );
    println!("  swapp_tag: {:?}", swapp_tag);
    println!("  p2id_tag: {:?}", p2id_tag);
    println!("  swap_count: {}", swap_count);
    println!("  creator_id: {:?}", creator_id);
    println!("  alice.id():  {:?}", alice.id());

    assert_eq!(creator_id, alice.id(), "Creator ID roundtrip failed!");
    assert_eq!(swap_count, 0, "Swap count should be 0");
    assert_eq!(
        requested_word[3].as_int(),
        25,
        "Requested amount should be 25"
    );
    println!("\nAll roundtrip checks passed!");
}
