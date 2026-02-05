# Coordinator API Documentation

Complete API reference for the Miden Swap Coordinator server.

## Base URL

```
http://localhost:3000
```

The server runs on `0.0.0.0:3000` by default.

---

## Endpoints

### 1. Submit Swap Note

Submit a new swap note for Bob (the solver) to process.

**Endpoint:** `POST /notes`

**Content-Type:** `application/json`

#### Request Body

```json
{
  "miden_note": "hex_encoded_note_bytes"
}
```

**Request Fields:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `miden_note` | `string` | Yes | Hex-encoded serialized Miden note bytes. This should be the complete serialized note that can be deserialized into a `Note` object. |

**Example Request:**

```bash
curl -X POST http://localhost:3000/notes \
  -H "Content-Type: application/json" \
  -d '{
    "miden_note": "a1b2c3d4e5f6..."
  }'
```

**Success Response (200 OK):**

```json
{
  "note_id": "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef"
}
```

**Response Fields:**

| Field | Type | Description |
|-------|------|-------------|
| `note_id` | `string` | The Miden note ID (hex-encoded). This is derived from the note itself and can be used to query the note's status. |

**Error Responses:**

| Status Code | Error Message | Description |
|-------------|---------------|-------------|
| `400 Bad Request` | `"Invalid hex string: ..."` | The `miden_note` field is not valid hex-encoded data. |
| `400 Bad Request` | `"Invalid note data: ..."` | The hex-decoded bytes cannot be deserialized into a valid Miden `Note` object. |

**Example Error Response:**

```json
{
  "error": "Invalid hex string: Invalid character 'x' at position 0"
}
```

**Notes:**

- The note ID is automatically extracted from the submitted note using `note.id().to_hex()`.
- The note is immediately added to the note pool with status `Pending`.
- Bob's solver will scan for this note every 5 seconds and attempt to execute the swap if viable.

---

### 2. Query Note Status

Query the current status of a submitted swap note.

**Endpoint:** `GET /notes/:id`

**URL Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `id` | `string` | Yes | The note ID (hex-encoded) returned from the submit endpoint. This is the path parameter in the URL. |

**Example Request:**

```bash
curl http://localhost:3000/notes/0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef
```

**Success Responses:**

The response format varies based on the note's status. All responses use a discriminated union with a `status` field.

#### Pending Status (200 OK)

Returned when the note is either `Pending` or `Claimed` (Bob is working on it).

```json
{
  "status": "pending"
}
```

**Response Fields:**

| Field | Type | Description |
|-------|------|-------------|
| `status` | `string` | Always `"pending"` for notes that haven't been completed yet. |

#### Solved Status (200 OK)

Returned when Bob has successfully executed the swap and created a P2ID note.

```json
{
  "status": "solved",
  "p2id_note": "hex_encoded_p2id_note_bytes"
}
```

**Response Fields:**

| Field | Type | Description |
|-------|------|-------------|
| `status` | `string` | Always `"solved"` for successfully completed swaps. |
| `p2id_note` | `string` | Hex-encoded serialized P2ID note bytes. This note can be consumed by Alice (the note creator) to receive the swapped assets. |

**Example Solved Response:**

```json
{
  "status": "solved",
  "p2id_note": "a1b2c3d4e5f6789012345678901234567890abcdef1234567890abcdef123456"
}
```

#### Failed Status (200 OK)

Returned when Bob attempted to execute the swap but it failed.

```json
{
  "status": "failed",
  "reason": "error_message"
}
```

**Response Fields:**

| Field | Type | Description |
|-------|------|-------------|
| `status` | `string` | Always `"failed"` for swaps that could not be executed. |
| `reason` | `string` | Human-readable error message explaining why the swap failed. |

**Common Failure Reasons:**

- `"Insufficient balance"` - Bob doesn't have enough of the requested asset in his vault.
- `"Swap execution failed: ..."` - The transaction execution failed on the Miden network.
- `"Unknown error"` - A failure occurred but no specific reason was captured.

**Example Failed Response:**

```json
{
  "status": "failed",
  "reason": "Insufficient balance"
}
```

**Error Responses:**

| Status Code | Error Message | Description |
|-------------|---------------|-------------|
| `404 Not Found` | `"Note not found"` | The provided note ID does not exist in the note pool. |

**Example Error Response:**

```json
{
  "error": "Note not found"
}
```

**Notes:**

- The `Pending` status is returned for both `Pending` and `Claimed` states (Bob is actively working on it).
- Once a note reaches `Solved` or `Failed`, its status will not change.
- The P2ID note in the `solved` response can be consumed by Alice using the Miden client.

---

## Note Status Lifecycle

```
┌─────────┐
│ Pending │  ← Note submitted via API
└────┬────┘
     │
     │ Bob claims the note
     ▼
┌──────────┐
│ Claimed  │  ← Bob is executing the swap
└────┬─────┘
     │
     ├─────────────────┐
     │                 │
     ▼                 ▼
┌─────────┐      ┌─────────┐
│ Solved  │      │ Failed  │
└─────────┘      └─────────┘
```

**Status Transitions:**

1. **Pending** → **Claimed**: Bob's solver atomically claims the note when it starts processing.
2. **Claimed** → **Solved**: Swap executed successfully, P2ID note created.
3. **Claimed** → **Failed**: Swap execution failed (insufficient balance, transaction error, etc.).
4. **Claimed** → **Pending**: Automatic timeout recovery (if note stays claimed > 5 minutes).

**Client-Facing Status:**

- `Pending` or `Claimed` → API returns `"status": "pending"`
- `Solved` → API returns `"status": "solved"` with `p2id_note`
- `Failed` → API returns `"status": "failed"` with `reason`

---

## Request/Response Examples

### Complete Workflow Example

#### Step 1: Submit a Swap Note

```bash
curl -X POST http://localhost:3000/notes \
  -H "Content-Type: application/json" \
  -d '{
    "miden_note": "a1b2c3d4e5f6789012345678901234567890abcdef1234567890abcdef123456"
  }'
```

**Response:**

```json
{
  "note_id": "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef"
}
```

#### Step 2: Poll for Status (Initially Pending)

```bash
curl http://localhost:3000/notes/0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef
```

**Response:**

```json
{
  "status": "pending"
}
```

#### Step 3: Poll Again (After Bob Processes - Solved)

```bash
curl http://localhost:3000/notes/0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef
```

**Response:**

```json
{
  "status": "solved",
  "p2id_note": "fedcba0987654321fedcba0987654321fedcba0987654321fedcba0987654321"
}
```

#### Step 4: Consume the P2ID Note

Use the `p2id_note` hex string with your Miden client to consume the note and receive the swapped assets.

---

## Error Handling

All endpoints return appropriate HTTP status codes:

- **200 OK**: Successful request
- **400 Bad Request**: Invalid request data (malformed hex, invalid note, etc.)
- **404 Not Found**: Note ID not found in pool
- **500 Internal Server Error**: Server-side error (not currently used, but possible)

Error responses are returned as plain text strings in the format:

```
"error_message"
```

---

## Rate Limiting

Currently, there is **no rate limiting** implemented. Clients can submit notes and query status as frequently as needed.

---

## Timeouts

- **Claimed Note Timeout**: 5 minutes (300 seconds)
  - If a note stays in `Claimed` state for more than 5 minutes, it's automatically reset to `Pending` to allow retry.
  - This provides crash recovery if Bob's solver crashes while processing a note.

---

## Data Formats

### Note Serialization

- **Input Format**: Hex-encoded bytes (string)
- **Output Format**: Hex-encoded bytes (string)
- **Serialization**: Uses Miden's `Note::read_from_bytes()` and `Note::write_to_bytes()` methods
- **Note ID**: Derived from the note itself using `note.id().to_hex()`

### Hex Encoding

- All hex strings use lowercase characters (`0-9a-f`)
- No `0x` prefix required (but accepted)
- Example: `"a1b2c3d4"` or `"0xa1b2c3d4"` (both valid)

---

## Implementation Details

### Note Pool

- **Storage**: In-memory `HashMap<String, NoteEntry>`
- **Thread Safety**: Uses `Arc<Mutex<>>` for concurrent access
- **Persistence**: None (all data lost on server restart)

### Bob Solver

- **Scan Interval**: Every 5 seconds
- **Processing**: Atomic claim → execute → commit cycle
- **Network Sync**: Syncs with Miden network before each scan

### API Server

- **Framework**: Axum (Rust)
- **Port**: 3000 (configurable in `main.rs`)
- **Binding**: `0.0.0.0` (accepts connections from any interface)

---

## Testing the API

### Using cURL

```bash
# Submit a note
NOTE_ID=$(curl -s -X POST http://localhost:3000/notes \
  -H "Content-Type: application/json" \
  -d '{"miden_note": "YOUR_NOTE_HEX"}' \
  | jq -r '.note_id')

# Query status
curl http://localhost:3000/notes/$NOTE_ID
```

### Using HTTPie

```bash
# Submit a note
http POST http://localhost:3000/notes miden_note="YOUR_NOTE_HEX"

# Query status
http GET http://localhost:3000/notes/NOTE_ID
```

### Using JavaScript/TypeScript

```javascript
// Submit a note
const response = await fetch('http://localhost:3000/notes', {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({
    miden_note: 'YOUR_NOTE_HEX'
  })
});
const { note_id } = await response.json();

// Poll for status
const statusResponse = await fetch(`http://localhost:3000/notes/${note_id}`);
const status = await statusResponse.json();

if (status.status === 'solved') {
  console.log('P2ID Note:', status.p2id_note);
} else if (status.status === 'failed') {
  console.error('Failed:', status.reason);
}
```

---

## Troubleshooting

### Note Not Found (404)

- **Cause**: Note ID doesn't exist in the pool
- **Solutions**:
  - Verify the note ID is correct (copy from submit response)
  - Check if the server was restarted (pool is in-memory)
  - Ensure the note was successfully submitted

### Invalid Hex String (400)

- **Cause**: `miden_note` field contains invalid hex characters
- **Solutions**:
  - Ensure the note is properly serialized to bytes first
  - Verify hex encoding is correct (only `0-9a-f` or `0-9A-F`)
  - Check for any whitespace or special characters

### Invalid Note Data (400)

- **Cause**: Hex-decoded bytes cannot be deserialized into a `Note`
- **Solutions**:
  - Verify the note was serialized correctly
  - Ensure you're using the complete note bytes, not just the note ID
  - Check that the note format matches Miden's expected structure

### Status Always Pending

- **Cause**: Bob hasn't processed the note yet
- **Solutions**:
  - Wait a few seconds (Bob scans every 5 seconds)
  - Check Bob's console logs for errors
  - Verify Bob has sufficient balance for the swap
  - Check if the note is properly formatted

---

## Security Considerations

⚠️ **Current Limitations (Demo Version):**

- ❌ No authentication/authorization
- ❌ No rate limiting
- ❌ No input validation beyond hex decoding
- ❌ No HTTPS (HTTP only)
- ❌ No CORS restrictions
- ❌ In-memory storage (data lost on restart)

**Production Recommendations:**

- Add authentication tokens
- Implement rate limiting
- Use HTTPS with TLS
- Add input validation and sanitization
- Implement persistent storage
- Add CORS policies
- Add request logging and monitoring

---

## Changelog

### Version 1.0.0

- Initial API implementation
- POST `/notes` - Submit swap notes
- GET `/notes/:id` - Query note status
- Bob solver background processing
- In-memory note pool
