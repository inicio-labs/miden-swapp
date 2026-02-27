use miden_crypto::utils::Deserializable;
use miden_mast_package::Package;
use miden_protocol::account::component::InitStorageData;
use miden_protocol::account::{
    Account, AccountBuilder, AccountComponent, AccountStorageMode, AccountType,
};
use miden_protocol::utils::sync::LazyLock;

use alloc::collections::BTreeSet;

// ACCOUNT COMPONENT
// ================================================================================================

const BASIC_WALLET_COMPONENT_BYTES: &[u8] =
    include_bytes!("../../contracts/basic-wallet/basic_wallet.masp");

/// Initialize the basic-wallet account component only once by loading the embedded package.
static BASIC_WALLET_COMPONENT: LazyLock<AccountComponent> = LazyLock::new(|| {
    let package = Package::read_from_bytes(BASIC_WALLET_COMPONENT_BYTES)
        .expect("Failed to deserialize basic-wallet package");

    let init_storage_data = InitStorageData::default();

    AccountComponent::from_package(&package, &init_storage_data)
        .expect("Failed to create account component from basic-wallet package")
        .with_supported_types(BTreeSet::from_iter([
            AccountType::RegularAccountImmutableCode,
            AccountType::RegularAccountUpdatableCode,
        ]))
});

// BASIC WALLET
// ================================================================================================

/// SDK-side representation of the basic-wallet account component.
///
/// This mirrors `contracts/basic-wallet/` and provides helpers for loading
/// the compiled component and building accounts that use it.
pub struct BasicWallet;

impl BasicWallet {
    /// Returns the loaded basic-wallet account component.
    pub fn component() -> AccountComponent {
        BASIC_WALLET_COMPONENT.clone()
    }

    /// Creates an account with the basic-wallet component and the given initial assets.
    ///
    /// # Arguments
    ///
    /// * `init_seed` - Seed bytes for the account builder
    /// * `storage_mode` - Account storage mode (e.g. `Public`)
    /// * `auth_component` - Authentication component for the account
    /// * `account_type` - Type of account to create
    pub fn create(
        init_seed: [u8; 32],
        storage_mode: AccountStorageMode,
        auth_component: impl Into<AccountComponent>,
        account_type: AccountType,
    ) -> Account {
        AccountBuilder::new(init_seed)
            .account_type(account_type)
            .storage_mode(storage_mode)
            .with_component(Self::component())
            .with_auth_component(auth_component)
            .build()
            .expect("Failed to build basic-wallet account")
    }
}

// TESTS
// ================================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_wallet() {
        let _component = BasicWallet::component();
    }
}
