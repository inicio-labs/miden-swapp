use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Status of a note in the pool
#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Pending,
    Claimed,
    Solved,
    Failed,
}

/// Entry for a note in the pool
#[derive(Debug, Clone)]
pub struct NoteEntry {
    pub raw_note_bytes: Vec<u8>,
    pub status: Status,
    pub p2id_note_bytes: Option<Vec<u8>>,
    pub created_at: Instant,
    pub claimed_at: Option<Instant>,
    pub failure_reason: Option<String>,
}

/// Thread-safe note pool for storing and managing notes
#[derive(Clone)]
pub struct NotePool {
    entries: Arc<Mutex<HashMap<String, NoteEntry>>>,
}

impl NotePool {
    /// Create a new empty note pool
    pub fn new() -> Self {
        Self {
            entries: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Insert a new pending note into the pool
    pub fn insert_pending(&self, note_id: String, raw_note_bytes: Vec<u8>) {
        let entry = NoteEntry {
            raw_note_bytes,
            status: Status::Pending,
            p2id_note_bytes: None,
            created_at: Instant::now(),
            claimed_at: None,
            failure_reason: None,
        };

        let mut entries = self.entries.lock().unwrap();
        entries.insert(note_id, entry);
    }

    /// Get a note entry by ID
    pub fn get(&self, note_id: &str) -> Option<NoteEntry> {
        let entries = self.entries.lock().unwrap();
        entries.get(note_id).cloned()
    }

    /// Get all pending notes
    pub fn get_pending_notes(&self) -> Vec<(String, NoteEntry)> {
        let entries = self.entries.lock().unwrap();
        entries
            .iter()
            .filter(|(_, entry)| entry.status == Status::Pending)
            .map(|(id, entry)| (id.clone(), entry.clone()))
            .collect()
    }

    /// Try to claim a note (atomically change from Pending to Claimed)
    /// Returns true if successful, false if already claimed or not found
    pub fn try_claim(&self, note_id: &str) -> bool {
        let mut entries = self.entries.lock().unwrap();
        
        if let Some(entry) = entries.get_mut(note_id) {
            if entry.status == Status::Pending {
                entry.status = Status::Claimed;
                entry.claimed_at = Some(Instant::now());
                return true;
            }
        }
        
        false
    }

    /// Mark a note as solved with the P2ID note result
    pub fn mark_solved(&self, note_id: &str, p2id_note_bytes: Vec<u8>) {
        let mut entries = self.entries.lock().unwrap();
        
        if let Some(entry) = entries.get_mut(note_id) {
            entry.status = Status::Solved;
            entry.p2id_note_bytes = Some(p2id_note_bytes);
        }
    }

    /// Mark a note as failed with an error reason
    pub fn mark_failed(&self, note_id: &str, reason: String) {
        let mut entries = self.entries.lock().unwrap();
        
        if let Some(entry) = entries.get_mut(note_id) {
            entry.status = Status::Failed;
            entry.failure_reason = Some(reason);
        }
    }

    /// Reset timed-out claimed notes back to pending
    /// This provides crash recovery for the solver
    pub fn reset_timed_out_claims(&self, timeout_secs: u64) {
        let mut entries = self.entries.lock().unwrap();
        let now = Instant::now();
        
        for entry in entries.values_mut() {
            if entry.status == Status::Claimed {
                if let Some(claimed_at) = entry.claimed_at {
                    if now.duration_since(claimed_at).as_secs() > timeout_secs {
                        println!("⚠️  [NotePool] Resetting timed-out note back to pending");
                        entry.status = Status::Pending;
                        entry.claimed_at = None;
                    }
                }
            }
        }
    }

    /// Get count of notes by status (for monitoring/logging)
    pub fn get_counts(&self) -> (usize, usize, usize, usize) {
        let entries = self.entries.lock().unwrap();
        let mut pending = 0;
        let mut claimed = 0;
        let mut solved = 0;
        let mut failed = 0;

        for entry in entries.values() {
            match entry.status {
                Status::Pending => pending += 1,
                Status::Claimed => claimed += 1,
                Status::Solved => solved += 1,
                Status::Failed => failed += 1,
            }
        }

        (pending, claimed, solved, failed)
    }
}

impl Default for NotePool {
    fn default() -> Self {
        Self::new()
    }
}
