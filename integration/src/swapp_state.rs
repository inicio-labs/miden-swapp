use anyhow::{Context, Result};
use miden_objects::account::AccountId;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize)]
pub struct SwappTestState {
    pub faucet1_id: String, // USDT faucet
    pub faucet2_id: String, // ETH faucet
    pub alice_id: String,
    pub bob_id: String,
}

impl SwappTestState {
    pub fn new(
        faucet1_id: AccountId,
        faucet2_id: AccountId,
        alice_id: AccountId,
        bob_id: AccountId,
    ) -> Self {
        // Convert AccountId to hex string using to_hex() method
        // This properly serializes the entire AccountId including version info
        Self {
            faucet1_id: faucet1_id.to_hex(),
            faucet2_id: faucet2_id.to_hex(),
            alice_id: alice_id.to_hex(),
            bob_id: bob_id.to_hex(),
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::state_file_path();
        let json = serde_json::to_string_pretty(self)?;
        fs::write(&path, json).context("Failed to write state file")?;
        println!("State saved to: {:?}", path);
        Ok(())
    }

    pub fn load() -> Result<Self> {
        let path = Self::state_file_path();
        let json = fs::read_to_string(&path)
            .context("Failed to read state file. Did you run the setup first?")?;
        let state: SwappTestState = serde_json::from_str(&json)?;
        println!("State loaded from: {:?}", path);
        Ok(state)
    }

    fn state_file_path() -> PathBuf {
        PathBuf::from("swapp_test_state.json")
    }

    pub fn faucet1_id(&self) -> Result<AccountId> {
        AccountId::from_hex(&self.faucet1_id).context("Failed to parse faucet1 ID")
    }

    pub fn faucet2_id(&self) -> Result<AccountId> {
        AccountId::from_hex(&self.faucet2_id).context("Failed to parse faucet2 ID")
    }

    pub fn alice_id(&self) -> Result<AccountId> {
        AccountId::from_hex(&self.alice_id).context("Failed to parse Alice ID")
    }

    pub fn bob_id(&self) -> Result<AccountId> {
        AccountId::from_hex(&self.bob_id).context("Failed to parse Bob ID")
    }
}
