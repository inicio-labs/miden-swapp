# Complete Explanation: Test File & Merkle Trees

## Part 1: Understanding Merkle Trees

### What is a Merkle Tree?

A **Merkle Tree** (also called a hash tree) is a data structure that:
- Organizes data in a tree-like structure
- Each leaf node contains data (or a hash of data)
- Each internal node is the hash of its children
- The root node is a single hash representing the entire tree

```
        Root Hash (H1234)
       /              \
   H12                H34
  /   \              /   \
H1    H2          H3    H4
|     |           |     |
D1   D2          D3    D4
```

**Key Properties:**
1. **Efficient Verification**: You can prove data exists without downloading the entire tree
2. **Tamper Detection**: Any change in data changes the root hash
3. **Partial Data Access**: You only need nodes along a path to verify a specific piece of data

### Use Cases of Merkle Trees

1. **Blockchain & Cryptocurrency**:
   - Bitcoin: Verifies transactions in blocks
   - Ethereum: Stores state (accounts, balances, contracts)
   - Miden: Stores account vaults (assets), storage, and other state

2. **Version Control Systems**:
   - Git uses Merkle trees to track file changes
   - Each commit is a root hash of the entire repository state

3. **Distributed Systems**:
   - Verify data integrity across multiple servers
   - Efficient synchronization (only sync changed branches)

4. **In Miden Specifically**:
   - **Asset Vaults**: Each account's assets are stored in a Sparse Merkle Tree (SMT)
   - **Storage**: Account storage slots are organized in Merkle trees
   - **Notes**: Note commitments use Merkle structures
   - **Proofs**: Merkle proofs allow verification without full data

### Sparse Merkle Tree (SMT) in Miden

A **Sparse Merkle Tree** is a special type where:
- Most leaves are empty (zero)
- Only non-zero leaves contain actual data
- Efficient for sparse data (like asset vaults with few assets)
- Depth is fixed (typically 64 levels in Miden)
- Each asset has a unique key (vault_key) that determines its position

**Example: Bob's Vault**
```
Root: 0xABC123...
├─ Path to ETH key (exists): [ETH: 25]
└─ Path to USDC key (empty): [empty]
```

When adding USDC, SMT_PEEK needs to traverse the path to the USDC key, even if it doesn't exist yet.

---

## Part 2: Complete Test File Explanation

### File Overview: `swapp_test.rs`

This file contains integration tests for a **swap note** system in Miden. It tests a decentralized exchange where users can swap assets via notes.

### Main Test: `swapp_note_full_fill_test`

#### **Step 1: Setup Faucets (Lines 23-41)**

```rust
let usdc_faucet = builder.add_existing_basic_faucet(...);
let eth_faucet = builder.add_existing_basic_faucet(...);
```

**What are Faucets?**
- Faucets are asset issuers in Miden
- They create fungible tokens (like USDC, ETH)
- Each faucet has:
  - `max_supply`: Maximum tokens that can be created
  - `total_issuance`: Current amount issued
  - `id()`: Unique identifier (prefix + suffix)

**In this test:**
- USDC faucet: max 1000, issued 150 (50 for note + 100 for Bob)
- ETH faucet: max 1000, issued 25 (for Alice's request)

#### **Step 2: Create Wallets (Lines 43-57)**

```rust
let alice = builder.add_existing_wallet_with_assets(
    [FungibleAsset::new(usdc_faucet.id(), 50)?.into()]
)?;
let bob = builder.add_existing_wallet_with_assets(
    [FungibleAsset::new(eth_faucet.id(), 25)?.into()]
)?;
```

**What are Wallets?**
- Wallets are accounts in Miden
- They have an `AssetVault` (a Sparse Merkle Tree storing assets)
- Each asset is stored at a position determined by its `vault_key`

**Initial State:**
- Alice: Has 50 USDC in her vault
- Bob: Has 25 ETH in his vault

#### **Step 3: Build Contract (Lines 59-65)**

```rust
let swapp_package = Arc::new(build_project_in_dir(
    Path::new("../contracts/swapp-note"),
    true,
)?);
```

**What is this?**
- Compiles the swap note contract from source
- Creates a `Package` containing the compiled code
- This code will execute when the note is consumed

#### **Step 4: Create Swap Note (Lines 67-99)**

```rust
let note_inputs = vec![
    eth_faucet.id().prefix().into(),  // Requested asset: ETH
    eth_faucet.id().suffix().into(),
    Felt::ZERO,
    Felt::new(25),                    // Amount: 25 ETH
    alice.id().prefix().into(),       // Note creator: Alice
    alice.id().suffix().into(),
    Felt::ZERO,
    Felt::ZERO,
];

let offered_asset = FungibleAsset::new(usdc_faucet.id(), 50)?;
let swap_note = create_testing_note_from_package(...);
```

**What is a Swap Note?**
- A note is a message/transaction in Miden
- Contains:
  - **Assets**: 50 USDC (what Alice offers)
  - **Inputs**: 25 ETH requested, Alice is creator
  - **Script**: The swap contract code
  - **Recipient**: Who can consume it (anyone in this case)

**The Swap Logic:**
- Alice creates a note offering 50 USDC
- The note says: "I want 25 ETH in exchange"
- Anyone can consume it by providing 25 ETH
- They receive the 50 USDC from the note

#### **Step 5: Execute Transaction (Lines 101-257)**

##### **5a. Prepare Note Arguments (Lines 113-123)**

```rust
let note_args = Word::from([
    Felt::ZERO,
    Felt::ZERO,
    Felt::ZERO,
    Felt::new(25), // input_amount = 25 (full fill)
]);
```

**What are Note Args?**
- Arguments passed to the note script when consumed
- `input_amount = 25` means Bob is providing 25 ETH (full fill)

##### **5b. Create Expected P2ID Note (Lines 125-205)**

```rust
let recipient = build_p2id_recipient(alice.id(), serial_num)?;
let p2id_note = Note::new(note_assets, note_metadata, recipient);
```

**What is a P2ID Note?**
- "Pay-to-ID" note: A note sent to a specific account ID
- This note will contain 25 ETH and be sent to Alice
- Created by the swap script when Bob consumes the swap note

##### **5c. Setup Merkle Store for SMT_PEEK (Lines 218-246)**

```rust
let bob_vault = tx_account.vault();

// Get nodes from Bob's actual vault (matching the vault root)
let bob_vault_inner_nodes: Vec<_> = bob_vault.inner_nodes().collect();

// Get witness for USDC key (doesn't exist yet, but we need the path)
let usdc_asset_to_add = FungibleAsset::new(usdc_faucet.id(), 50)?;
let usdc_witness = bob_vault.open(usdc_asset_to_add.vault_key());

// Combine nodes from vault and witness path
let mut all_inner_nodes: Vec<_> = bob_vault_inner_nodes;
all_inner_nodes.extend(usdc_witness.authenticated_nodes());

advice_inputs.store.extend(all_inner_nodes);
```

**Why is this needed?**

When the swap script executes `active_note::add_assets_to_account()`, it:
1. Takes the 50 USDC from the note
2. Adds it to Bob's vault
3. Calls `asset_vault::add_asset` which uses `SMT_PEEK`

**SMT_PEEK Operation:**
- `SMT_PEEK` reads a node from the Merkle tree
- It needs the tree structure in the `MerkleStore` (advice provider)
- When adding USDC to Bob's vault:
  - Bob's vault currently only has ETH
  - USDC key doesn't exist yet
  - But `SMT_PEEK` needs to traverse the path to the USDC key
  - This requires all intermediate nodes along that path

**The Solution:**
1. Get all inner nodes from Bob's existing vault (for ETH)
2. Get the witness for the USDC key path (even though USDC doesn't exist)
3. The witness contains all nodes needed to traverse to the USDC key position
4. Add all these nodes to the Merkle store

**Merkle Tree Visualization:**

```
Bob's Vault Root (before):
├─ Path to ETH key: [ETH: 25] ← exists
└─ Path to USDC key: [empty]  ← doesn't exist, but path exists

When adding USDC:
├─ Path to ETH key: [ETH: 25] ← still exists
└─ Path to USDC key: [USDC: 50] ← now exists
```

The witness for USDC key gives us all the intermediate nodes (siblings) along the path, even though the leaf is empty.

##### **5d. Execute Transaction (Lines 248-257)**

```rust
let tx_context = mock_chain
    .build_tx_context(bob.id(), &[swap_note.id()], &[])?
    .extend_note_args(note_args_map)
    .extend_expected_output_notes(vec![OutputNote::Full(p2id_note.into())])
    .extend_advice_map(AdviceInputs::default())
    .build()?;

let executed_transaction = tx_context.execute().await?;
```

**What happens:**
1. Bob consumes the swap note
2. Swap script executes:
   - Validates Bob provided 25 ETH (full fill)
   - Adds 50 USDC from note to Bob's vault (requires SMT_PEEK)
   - Creates P2ID note with 25 ETH for Alice
3. Transaction completes

**Note:** Lines 260-336 contain verification code (commented out) that would check:
- P2ID note was created correctly
- Bob received 50 USDC
- Bob spent 25 ETH

---

### Other Tests in the File

#### **Test 2: `swapp_note_creator_reclaim_test` (Lines 500-607)**

Tests when Alice (note creator) consumes her own note:
- Should receive assets back (50 USDC)
- No P2ID note created
- No remainder note created

#### **Test 3: `swapp_note_invalid_input_test` (Lines 609-702)**

Tests error handling:
- Bob tries to provide more than requested (30 ETH > 25 ETH)
- Transaction should fail with assertion error

#### **Test 4: `swapp_note_partial_fill_test` (Lines 339-498, commented out)**

Tests partial fills:
- Bob provides 15 ETH (60% of 25 requested)
- Should create:
  - P2ID note with 15 ETH for Alice
  - Remainder swap note with 20 USDC (still requesting 10 ETH)

---

## Part 3: Key Concepts Summary

### Merkle Trees in This Context

1. **Asset Vault = Sparse Merkle Tree**
   - Each account has a vault (SMT)
   - Assets are stored at positions determined by `vault_key`
   - Root hash represents the entire vault state

2. **SMT_PEEK Operation**
   - Reads a node from the tree during execution
   - Needs the tree structure in MerkleStore
   - Must have all nodes along the path to the target key

3. **Witness/Proof**
   - A proof that a key exists (or doesn't exist) in the tree
   - Contains all sibling nodes along the path
   - Allows verification without full tree

4. **Why We Need to Extend Merkle Store**
   - When adding a new asset, SMT_PEEK needs to read the current state
   - The path to the new asset key might not exist yet
   - We provide the witness path so SMT_PEEK can traverse it

### The Problem We Solved

**Original Issue:**
- Error: "SMT node not found"
- When adding USDC to Bob's vault, SMT_PEEK couldn't find nodes along the USDC key path

**Solution:**
- Get witness for USDC key from Bob's actual vault
- This witness contains all nodes needed for the path
- Add these nodes to the Merkle store before execution
- Now SMT_PEEK can traverse the path successfully

---

## Part 4: Code Flow Diagram

```
1. Setup
   ├─ Create USDC faucet
   ├─ Create ETH faucet
   ├─ Create Alice wallet (50 USDC)
   └─ Create Bob wallet (25 ETH)

2. Create Swap Note
   ├─ Alice creates note: "50 USDC for 25 ETH"
   └─ Note added to genesis

3. Bob Consumes Note
   ├─ Bob provides 25 ETH
   ├─ Swap script executes:
   │  ├─ Validates input
   │  ├─ Adds 50 USDC to Bob's vault (SMT_PEEK needed)
   │  └─ Creates P2ID note (25 ETH for Alice)
   └─ Transaction completes

4. Result
   ├─ Bob: +50 USDC, -25 ETH
   └─ Alice: Receives P2ID note with 25 ETH
```

---

## Part 5: Merkle Tree Deep Dive

### How SMT_PEEK Works

1. **Input**: Root hash, depth, index (key position)
2. **Process**:
   - Start at root
   - Traverse down using index bits
   - At each level, read the node from MerkleStore
   - If node missing → error ("SMT node not found")
3. **Output**: Node value at the specified position

### Why We Need the Witness

When adding USDC to Bob's vault:
- USDC key position: `0x1234...` (determined by faucet ID)
- ETH key position: `0x5678...` (different position)
- Paths diverge at some level

**Without witness:**
- SMT_PEEK tries to traverse to USDC key
- Reaches a level where the node is missing
- Error: "SMT node not found"

**With witness:**
- Witness provides all sibling nodes along USDC path
- SMT_PEEK can traverse successfully
- Can read current value (empty) and update to USDC

### Inner Nodes vs Leaves

- **Inner Nodes**: Non-leaf nodes in the tree (used for traversal)
- **Leaves**: Actual data (assets in this case)
- **Witness**: Contains inner nodes along a path (siblings)

In our code:
- `bob_vault.inner_nodes()`: All inner nodes for existing assets (ETH)
- `usdc_witness.authenticated_nodes()`: Inner nodes along USDC path
- Combined: All nodes needed for SMT_PEEK to work

---

This completes the explanation of the test file and Merkle trees in the context of Miden!


