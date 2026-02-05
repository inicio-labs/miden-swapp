# Miden Swap Coordinator Server

A minimal Rust backend coordinator for the Miden swap system with HTTP APIs where Alice submits Miden notes, a background Bob solver continuously scans and executes swaps, and Alice polls for P2ID results.

## Architecture

The coordinator consists of:

1. **HTTP API Server** - Accepts note submissions from Alice and provides status queries
2. **In-Memory Note Pool** - Stores submitted notes with their current status
3. **Bob Solver Loop** - Background task that continuously:
   - Scans API-submitted notes every 5 seconds
   - Checks Bob's balance and swap feasibility
   - Executes viable swaps on the Miden network
   - Scans and consumes Bob's network consumable notes (P2ID notes, minted notes, etc.)

## Prerequisites

1. **Faucets deployed**: Run `cargo run --bin swapp_setup` to deploy USDT and ETH faucets
2. **Basic-wallet contract**: Must be built (located at `contracts/basic-wallet/`)
3. **Swap-note contract**: Must be built (located at `contracts/swapp-note/`)

## Quick Start

### 1. Start the Coordinator

```bash
cd coordinator
cargo run --release
```

The server will:
- Initialize a Miden client
- Create Bob's solver account
- Display Bob's Account ID prominently
- Start the HTTP server on `0.0.0.0:3000`
- Begin Bob's scanning loop (every 5 seconds)

**Important**: Copy Bob's Account ID from the console output!

Example output:
```
========================================
🤖 BOB SOLVER ACCOUNT CREATED
========================================
Account ID: 0x123abc...
Account ID (hex): ...
========================================
```

### 2. Fund Bob's Account

Bob needs tokens to fulfill swaps. Use the displayed Account ID to mint tokens:

```bash
# Mint USDT to Bob
cargo run --bin swapp_setup  # or use your faucet minting script

# Mint ETH to Bob
# (repeat for different token types)
```

### 3. Submit a Swap Note (as Alice)

```bash
curl -X POST http://localhost:3000/notes \
  -H "Content-Type: application/json" \
  -d '{"miden_note": "YOUR_HEX_ENCODED_NOTE_HERE"}'
```

Response:
```json
{
  "note_id": "550e8400-e29b-41d4-a716-446655440000"
}
```

### 4. Poll for Results

```bash
curl http://localhost:3000/notes/550e8400-e29b-41d4-a716-446655440000
```

Responses:

**Pending:**
```json
{
  "status": "pending"
}
```

**Solved:**
```json
{
  "status": "solved",
  "p2id_note": "HEX_ENCODED_P2ID_NOTE"
}
```

**Failed:**
```json
{
  "status": "failed",
  "reason": "Insufficient balance"
}
```

## API Endpoints

### POST /notes

Submit a new swap note for Bob to solve.

**Request:**
```json
{
  "miden_note": "hex_encoded_note_bytes"
}
```

**Response:**
```json
{
  "note_id": "uuid"
}
```

### GET /notes/:id

Query the status of a submitted note.

**Response (Pending):**
```json
{
  "status": "pending"
}
```

**Response (Solved):**
```json
{
  "status": "solved",
  "p2id_note": "hex_encoded_p2id_note"
}
```

**Response (Failed):**
```json
{
  "status": "failed",
  "reason": "error_message"
}
```

## Bob Solver Behavior

Every 5 seconds, Bob performs two scans:

### Scan 1: API-Submitted Notes

1. **Discover**: Find all pending notes in the pool
2. **Pre-check**: Verify Bob has sufficient balance for the swap
3. **Claim**: Atomically mark the note as "Claimed"
4. **Execute**: Consume the swap note on Miden and create P2ID note
5. **Commit**: Update pool with result (Solved or Failed)

### Scan 2: Network Consumable Notes

- Automatically consumes any notes Bob can consume (P2ID notes, minted notes, etc.)
- Keeps Bob's vault updated with received assets

## Console Logging

The coordinator provides extensive logging for demo visibility:

```
🔍 [Bob Solver] Scanning cycle started...
✓ [Bob Solver] Synced with network
📋 [Bob Solver] Checking API-submitted notes in pool...
   Found 1 pending API notes
   🔎 Examining note abc123...
      Bob's vault: 2 assets
      ✓ Swap is viable
      🔒 Claimed note abc123
      ⚙️  Executing swap transaction...
         TX ID: 0x...
      ✅ Swap executed successfully!
         P2ID note created: 0x...
🌐 [Bob Solver] Checking network consumable notes...
   Found 2 consumable notes on network
   Note IDs: [0x..., 0x...]
   💰 Consuming all consumable notes...
   ✅ Consumed 2 notes. TX: 0x...
⏸️  [Bob Solver] Scan complete. Sleeping for 5 seconds...
```

## Configuration

### Keystore and Store Paths

The coordinator uses paths configured in `integration/src/helpers.rs`:
- Keystore: `/Users/vaibhavjindal/miden-swapp/integration/keystore`
- SQLite Store: `/Users/vaibhavjindal/miden-swapp/integration/store.sqlite3`

### Timeout Settings

- Claimed note timeout: 5 minutes (300 seconds)
- Scan interval: 5 seconds
- Port: 3000

## Architecture Details

### Note Pool State Machine

```
Pending → Claimed → Solved
                 ↘ Failed
```

- **Pending**: Note submitted but not yet claimed
- **Claimed**: Bob is actively working on this note
- **Solved**: Swap executed successfully, P2ID note available
- **Failed**: Swap execution failed

### Crash Recovery

If a note stays in "Claimed" state for more than 5 minutes, it's automatically reset to "Pending" to allow retry.

## Non-Goals (Demo Simplicity)

- ❌ No persistence (in-memory HashMap only)
- ❌ No multiple solvers (single Bob)
- ❌ No profitability logic
- ❌ No WebSockets (polling only)
- ❌ No authentication/authorization
- ❌ No metrics/monitoring

## Troubleshooting

### Bob Can't Execute Swaps

**Symptom**: Logs show "❌ Not viable - insufficient balance"

**Solution**: Fund Bob's account with the required tokens

### Notes Stuck in Pending

**Symptom**: Status always returns "pending"

**Solution**: 
1. Check Bob's console logs for errors
2. Verify Bob has sufficient balance
3. Check if the note is properly formatted

### Server Won't Start

**Symptom**: Port 3000 already in use

**Solution**: Kill existing process or change port in `main.rs`

## Development

### Building

```bash
cargo build --package coordinator
```

### Running in Debug Mode

```bash
cargo run --package coordinator
```

### Testing

The coordinator integrates with the existing swap test infrastructure:

```bash
# Run integration tests
cd ../integration
cargo test
```

## File Structure

```
coordinator/
├── Cargo.toml           # Dependencies
├── README.md            # This file
└── src/
    ├── main.rs          # Server entry point
    ├── api.rs           # HTTP routes (Axum)
    ├── note_pool.rs     # In-memory state (Arc<Mutex<HashMap>>)
    ├── solver.rs        # Bob background loop
    └── lib.rs           # Module exports
```

## License

Same as parent project.
