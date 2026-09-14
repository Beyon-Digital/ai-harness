//! macOS Keychain backend.
//!
//! [`MacOsKeychainSecretStore`] is compiled only on macOS. It calls
//! `security-framework` directly with byte slices, so secret values are never
//! interpolated into a command line (R4.7). Each secret owns a generic
//! password item under `service`/`uri` plus a companion metadata item under
//! `service`/`uri#metadata`. Backend failures are stable `Unavailable` errors
//! with no fallback to plaintext.

#[cfg(target_os = "macos")]
mod macos {
    use async_trait::async_trait;
    use errors::KernelError;
    use errors::codes::{ErrorCode, RetryClass};
    use security_framework::base::Error as KeychainError;
    use security_framework::passwords;

    use crate::{ActResult, SecretMetadata, SecretStore, SecretValue};

    /// `errSecItemNotFound` from the Security framework.
    const ERR_SEC_ITEM_NOT_FOUND: i32 = -25_300;

    /// Suffix of the companion metadata item's account.
    const METADATA_SUFFIX: &str = "#metadata";

    /// Version header stored in the companion metadata item.
    const METADATA_HEADER: &str = "agentd-secrets-v1";

    /// Keychain-backed secret store scoped to one dedicated service name.
    #[derive(Clone, Debug)]
    pub struct MacOsKeychainSecretStore {
        service: String,
    }

    impl MacOsKeychainSecretStore {
        /// Creates a store over a dedicated Keychain service name.
        pub fn new(service: impl Into<String>) -> Self {
            Self {
                service: service.into(),
            }
        }

        /// Returns the Keychain service name.
        pub fn service(&self) -> &str {
            &self.service
        }

        /// Creates or replaces the items backing `metadata.uri`.
        ///
        /// Secret creation and rotation are outside the broker's scope; this
        /// is the backend seam operators and opt-in tests use to provision
        /// their own item before exercising the contract.
        pub fn put(&self, metadata: &SecretMetadata, value: &[u8]) -> errors::Result<()> {
            let encoded = encode_metadata(metadata)?;
            passwords::set_generic_password(&self.service, &metadata.uri, value)
                .map_err(|_| write_failed("keychain secret write failed"))?;
            passwords::set_generic_password(
                &self.service,
                &metadata_account(&metadata.uri),
                &encoded,
            )
            .map_err(|_| write_failed("keychain metadata write failed"))
        }

        /// Deletes both items for `uri`; missing items are not an error.
        pub fn delete(&self, uri: &str) -> errors::Result<()> {
            for account in [uri.to_string(), metadata_account(uri)] {
                if let Err(error) = passwords::delete_generic_password(&self.service, &account)
                    && error.code() != ERR_SEC_ITEM_NOT_FOUND
                {
                    return Err(write_failed("keychain delete failed"));
                }
            }
            Ok(())
        }
    }

    #[async_trait]
    impl SecretStore for MacOsKeychainSecretStore {
        async fn metadata(&self, uri: &str) -> errors::Result<SecretMetadata> {
            let encoded = passwords::get_generic_password(&self.service, &metadata_account(uri))
                .map_err(|error| read_failed("secret metadata not found", error))?;
            decode_metadata(uri, &encoded)
        }

        async fn get(&self, uri: &str) -> errors::Result<SecretValue> {
            passwords::get_generic_password(&self.service, uri)
                .map(SecretValue::new)
                .map_err(|error| read_failed("secret not found", error))
        }

        async fn sign_or_act(
            &self,
            _uri: &str,
            _action: &str,
            _payload: &[u8],
        ) -> errors::Result<ActResult> {
            Err(KernelError::new(
                ErrorCode::FailedPrecondition,
                RetryClass::Never,
                "sign-or-act is not supported by the macOS Keychain backend",
            ))
        }
    }

    /// Maps a read failure: absent items are `NotFound`, everything else is a
    /// stable `Unavailable` with no fallback.
    fn read_failed(not_found: &'static str, error: KeychainError) -> KernelError {
        if error.code() == ERR_SEC_ITEM_NOT_FOUND {
            KernelError::new(ErrorCode::NotFound, RetryClass::Never, not_found)
        } else {
            KernelError::new(
                ErrorCode::Unavailable,
                RetryClass::Safe,
                "keychain read failed",
            )
        }
    }

    /// Builds a stable write/delete failure.
    fn write_failed(message: &'static str) -> KernelError {
        KernelError::new(ErrorCode::Unavailable, RetryClass::Safe, message)
    }

    /// Builds the companion metadata item's account name.
    fn metadata_account(uri: &str) -> String {
        format!("{uri}{METADATA_SUFFIX}")
    }

    /// Encodes metadata as `header\nkind\nscope...`.
    fn encode_metadata(metadata: &SecretMetadata) -> errors::Result<Vec<u8>> {
        if metadata.kind.contains('\n') || metadata.scopes.iter().any(|scope| scope.contains('\n'))
        {
            return Err(KernelError::new(
                ErrorCode::InvalidArgument,
                RetryClass::Never,
                "secret metadata contains a line break",
            ));
        }
        let mut encoded = String::from(METADATA_HEADER);
        encoded.push('\n');
        encoded.push_str(&metadata.kind);
        for scope in &metadata.scopes {
            encoded.push('\n');
            encoded.push_str(scope);
        }
        Ok(encoded.into_bytes())
    }

    /// Decodes the companion metadata item; corruption is an internal error.
    fn decode_metadata(uri: &str, encoded: &[u8]) -> errors::Result<SecretMetadata> {
        let text = std::str::from_utf8(encoded).map_err(|_| corrupt_metadata())?;
        let mut lines = text.split('\n');
        if lines.next() != Some(METADATA_HEADER) {
            return Err(corrupt_metadata());
        }
        let kind = lines.next().ok_or_else(corrupt_metadata)?;
        if kind.is_empty() {
            return Err(corrupt_metadata());
        }
        Ok(SecretMetadata {
            uri: uri.to_string(),
            kind: kind.to_string(),
            scopes: lines.map(str::to_string).collect(),
        })
    }

    /// Builds the stable malformed-metadata error.
    fn corrupt_metadata() -> KernelError {
        KernelError::new(
            ErrorCode::Internal,
            RetryClass::Never,
            "keychain metadata item is malformed",
        )
    }
}

#[cfg(target_os = "macos")]
pub use macos::MacOsKeychainSecretStore;
