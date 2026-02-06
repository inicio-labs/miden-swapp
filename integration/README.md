# Swap Note Integration

This directory contains integration tests and binaries for the Miden swap note contract.

## Structure

```
integration/
├── src/
│   ├── bin/
│   │   └── swapp_note.rs    # End-to-end swap integration binary
│   ├── helpers.rs            # Helper functions for testing
│   └── lib.rs                # Library exports
├── tests/
│   └── swapp_test.rs         # Integration tests
└── Cargo.toml
```

## Swap Note Binary

The `swapp_note.rs` binary demonstrates a complete end-to-end swap scenario:

### Scenario Overview

1. **Setup**: Two faucets (USDT and ETH) and two wallet accounts (Alice and Bob)
2. **Minting**: Each faucet mints 100 tokens to their respective account
3. **Swap Creation**: Alice creates a swap note offering 50 USDT for 75 ETH
4. **Partial Fill**: Bob provides 40 ETH and receives ~26.67 USDT
5. **Full Fill**: Bob provides the remaining 35 ETH and receives ~23.33 USDT
6. **P2ID Routing**: Alice receives the ETH via P2ID routing notes

### Running the Binary

```bash
cd integration
cargo run --bin swapp_note
```

### Expected Output

The binary will print detailed logs for each step:
- Faucet and account creation
- Token minting and consumption
- Swap note creation and publication
- Partial fill transaction
- Full fill transaction
- Final balances

### Expected Final State

- **Alice**: 
  - Started with: 100 USDT
  - Offered: 50 USDT
  - Received: 75 ETH (via two P2ID notes: 40 + 35)
  - Final: 50 USDT + 75 ETH

- **Bob**:
  - Started with: 100 ETH
  - Spent: 75 ETH (40 + 35)
  - Received: ~50 USDT (26.67 + 23.33)
  - Final: 25 ETH + ~50 USDT

### Important Notes

1. **Note Args Limitation**: The current implementation uses `build_consume_notes` without explicit note args. The swap note contract expects note args in the format `[input_amount, 0, 0, 0]`. This limitation exists because the miden-client v0.12 API may not fully support passing note arguments during consumption.

2. **Testnet Delays**: The binary includes delays between steps to allow for testnet block confirmations. Adjust the `Duration::from_secs()` values if needed.

3. **Authentication**: The binary uses `AuthRpoFalcon512` for all accounts with automatically generated keypairs stored in the keystore.

## Integration Tests

The `swapp_test.rs` file contains two test scenarios:

### Test 1: Basic Swap Note Structure

Tests that:
- Swap notes can be created with correct inputs
- Note args can be configured
- Transaction context builds successfully

```bash
cargo test --package integration swapp_note_basic_test -- --nocapture
```

### Test 2: Creator Reclaim Scenario

Tests that:
- The note creator can consume their own note
- All assets are returned to the creator
- No swap logic is executed (creator = consumer path)

```bash
cargo test --package integration swapp_note_creator_can_reclaim -- --nocapture
```

### Running All Tests

```bash
cargo test --package integration -- --nocapture
```

## Helper Functions

The `helpers.rs` module provides:

- `setup_client()`: Initializes the Miden client with keystore and RPC connection
- `build_project_in_dir()`: Compiles Miden contracts
- `create_testing_account_from_package()`: Creates accounts from compiled packages
- `create_testing_note_from_package()`: Creates notes from compiled packages

## Dependencies

Key dependencies (defined in `Cargo.toml`):
- `miden-client`: v0.12 (with `tonic` and `testing` features)
- `miden-lib`: v0.12 (for account auth components)
- `miden-objects`: v0.12 (with `testing` features)
- `miden-testing`: v0.12 (for MockChain)
- `tokio`: v1.40 (async runtime)

## Swap Note Contract

The swap note contract (`../contracts/swapp-note/src/lib.rs`) implements:

- **Partial fills**: Consumer can provide less than the requested amount
- **Proportional output**: Output calculated as `(input_amount * offered_total) / requested_total`
- **Remainder notes**: Automatically created for partial fills
- **P2ID routing**: Requested assets are routed to the creator via P2ID notes
- **Creator reclaim**: Creator can consume their own note to reclaim assets

### Note Inputs Structure

The swap note expects 8 Felts as inputs:

```rust
[
    // Positions 0-3: Requested Asset Word
    requested_asset_id_prefix,
    requested_asset_id_suffix,
    0 (padding),
    requested_asset_total,
    
    // Positions 4-7: Creator AccountId
    creator_account_id_prefix,
    creator_account_id_suffix,
    0 (padding),
    0 (padding),
]
```

### Note Args Structure

When consuming the swap note, the consumer should provide:

```rust
[
    input_amount,  // Amount of requested asset being provided
    0,             // Unused
    0,             // Unused
    0,             // Unused
]
```

## Troubleshooting

### Asset Creation Issues

If you encounter `FungibleFaucetIdTypeMismatch` errors, ensure you're using V1 AccountIds (faucet accounts) for creating fungible assets, not V0 AccountIds (wallet accounts).

### Note Args Not Working

The current API version may have limitations with passing note args. Ensure you're using the correct method for your miden-client version. Check the official Miden documentation for updates.

### Testnet Sync Issues

If notes don't appear as consumable, try:
1. Increasing the sleep duration between steps
2. Running `client.sync_state().await?` more frequently
3. Checking the testnet block explorer for transaction status

## Future Improvements

1. **Note Args Support**: Update to use the proper API for passing note arguments once available
2. **Account Balance Verification**: Add code to query and verify final account balances
3. **Error Handling**: Add more detailed error messages and recovery logic
4. **Configurable Amounts**: Accept command-line arguments for swap amounts
5. **Multi-hop Swaps**: Demonstrate more complex swap scenarios

