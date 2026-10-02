use crate::CredentialStoreError;
use crate::KeyringStore;

/// OHOS has no verified native credential backend yet. Never let keyring's
/// in-memory default report a successful persistent credential write.
#[derive(Debug, Clone, Copy)]
pub struct UnsupportedKeyringStore;

fn unavailable() -> CredentialStoreError {
    CredentialStoreError::new(keyring::Error::PlatformFailure(Box::new(
        std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "native credential storage is not yet supported on HarmonyOS; configure file storage explicitly",
        ),
    )))
}

impl KeyringStore for UnsupportedKeyringStore {
    fn load(&self, _service: &str, _account: &str) -> Result<Option<String>, CredentialStoreError> {
        Err(unavailable())
    }

    fn save(
        &self,
        _service: &str,
        _account: &str,
        _value: &str,
    ) -> Result<(), CredentialStoreError> {
        Err(unavailable())
    }

    fn delete(&self, _service: &str, _account: &str) -> Result<bool, CredentialStoreError> {
        Err(unavailable())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_backend_never_reports_persistence_or_missing_credentials() {
        let store = UnsupportedKeyringStore;
        let errors = [
            store
                .save("service", "account", "private credential")
                .unwrap_err(),
            store.load("service", "account").unwrap_err(),
            store.delete("service", "account").unwrap_err(),
        ];
        for error in errors {
            assert!(!error.message().contains("private credential"));
            let error = std::io::Error::from(error);
            assert_eq!(error.kind(), std::io::ErrorKind::Unsupported);
        }
    }
}
