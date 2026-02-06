use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
    routing::{get, post},
    Router,
};
use miden_client::{
    note::{Note, NoteFile},
    utils::Deserializable,
};
use serde::{Deserialize, Serialize};
use tower_http::cors::{Any, CorsLayer};

use crate::note_pool::{NotePool, Status};

/// Request body for submitting a note
#[derive(Debug, Deserialize)]
pub struct SubmitNoteRequest {
    pub miden_note: String, // Hex-encoded note bytes
}

/// Response for note submission
#[derive(Debug, Serialize)]
pub struct SubmitNoteResponse {
    pub note_id: String,
}

/// Response for note status query
#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum NoteStatusResponse {
    Pending,
    Solved { p2id_note: String },
    Failed { reason: String },
}

/// Handler for POST /notes - Submit a new swap note
async fn submit_note(
    State(note_pool): State<NotePool>,
    Json(payload): Json<SubmitNoteRequest>,
) -> Result<Json<SubmitNoteResponse>, (StatusCode, String)> {
    // Decode hex string to bytes
    let note_bytes = hex::decode(&payload.miden_note).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!("Invalid hex string: {}", e),
        )
    })?;

    // Deserialize the note to get its ID
    let note = Note::read_from_bytes(&note_bytes)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid note data: {}", e)))?;

    // Use the actual Miden note ID (convert to hex string)
    let note_id = note.id().to_hex();

    // Insert into note pool
    note_pool.insert_pending(note_id.clone(), note_bytes);

    // Log the submission
    println!("📥 [API] New note submitted: {}", note_id);

    Ok(Json(SubmitNoteResponse { note_id }))
}

/// Handler for GET /notes/:id - Query note status
async fn get_note_status(
    State(note_pool): State<NotePool>,
    Path(note_id): Path<String>,
) -> Result<Json<NoteStatusResponse>, (StatusCode, String)> {
    // Get note from pool
    let entry = note_pool
        .get(&note_id)
        .ok_or((StatusCode::NOT_FOUND, "Note not found".to_string()))?;

    // Build response based on status
    let response = match entry.status {
        Status::Pending | Status::Claimed => {
            // Both Pending and Claimed appear as "pending" to the client
            println!("📤 [API] Status query for note: {} -> pending", note_id);
            NoteStatusResponse::Pending
        }
        Status::Solved => {
            let p2id_note_hex = hex::encode(entry.p2id_note_bytes.unwrap_or_default());
            println!("📤 [API] Status query for note: {} -> solved", note_id);
            NoteStatusResponse::Solved {
                p2id_note: p2id_note_hex,
            }
        }
        Status::Failed => {
            let reason = entry
                .failure_reason
                .unwrap_or_else(|| "Unknown error".to_string());
            println!("📤 [API] Status query for note: {} -> failed", note_id);
            NoteStatusResponse::Failed { reason }
        }
    };

    Ok(Json(response))
}

/// Create the API router with all routes
pub fn create_api_routes(note_pool: NotePool) -> Router {
    // Configure CORS to allow requests from any origin
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        .route("/notes", post(submit_note))
        .route("/notes/:id", get(get_note_status))
        .layer(cors)
        .with_state(note_pool)
}
