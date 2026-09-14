use serde::Serialize;
use std::collections::BTreeMap;

/// Stable error payload exposed across the Tauri IPC boundary.
///
/// `debug_message` is deliberately sanitized. Raw errors, file paths, URLs,
/// tokens, and stack traces belong in Rust logs and must never be copied here.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NativeError {
    pub code: String,
    pub params: BTreeMap<String, String>,
    pub debug_message: String,
}

impl NativeError {
    pub fn new(code: &'static str) -> Self {
        let registration =
            registration(code).unwrap_or_else(|| registration("NATIVE_UNKNOWN").unwrap());
        Self {
            code: registration.code.to_string(),
            params: BTreeMap::new(),
            debug_message: registration.safe_debug_message.to_string(),
        }
    }

    pub fn with_param(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.params.insert(name.into(), value.into());
        self
    }

    pub fn translation_key(&self) -> &'static str {
        registration(&self.code)
            .unwrap_or_else(|| registration("NATIVE_UNKNOWN").unwrap())
            .translation_key
    }
}

#[derive(Debug, Clone, Copy)]
pub struct NativeErrorRegistration {
    pub code: &'static str,
    pub translation_key: &'static str,
    pub safe_debug_message: &'static str,
}

pub const NATIVE_ERROR_REGISTRY: &[NativeErrorRegistration] = &[
    NativeErrorRegistration {
        code: "NATIVE_UNKNOWN",
        translation_key: "error.nativeUnknown",
        safe_debug_message: "Unexpected native operation failure",
    },
    NativeErrorRegistration {
        code: "I18N_INVALID_LOCALE",
        translation_key: "error.invalidLocale",
        safe_debug_message: "Rejected unsupported UI locale",
    },
    NativeErrorRegistration {
        code: "I18N_STATE_UNAVAILABLE",
        translation_key: "error.localeStateUnavailable",
        safe_debug_message: "Native locale state was unavailable",
    },
    NativeErrorRegistration {
        code: "I18N_PERSISTENCE_FAILED",
        translation_key: "error.localePersistenceFailed",
        safe_debug_message: "Native locale preference persistence failed",
    },
    NativeErrorRegistration {
        code: "NOTIFICATION_MANAGER_UNAVAILABLE",
        translation_key: "error.notificationManagerUnavailable",
        safe_debug_message: "Notification manager was not initialized",
    },
    NativeErrorRegistration {
        code: "NOTIFICATION_OPERATION_FAILED",
        translation_key: "error.notificationOperationFailed",
        safe_debug_message: "Notification operation failed",
    },
    NativeErrorRegistration {
        code: "NOTIFICATION_SERIALIZATION_FAILED",
        translation_key: "error.notificationSerializationFailed",
        safe_debug_message: "Notification statistics serialization failed",
    },
];

pub fn registration(code: &str) -> Option<&'static NativeErrorRegistration> {
    NATIVE_ERROR_REGISTRY
        .iter()
        .find(|entry| entry.code == code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_codes_and_translation_keys_are_unique() {
        let mut codes = std::collections::BTreeSet::new();
        let mut keys = std::collections::BTreeSet::new();
        for entry in NATIVE_ERROR_REGISTRY {
            assert!(
                codes.insert(entry.code),
                "duplicate error code: {}",
                entry.code
            );
            assert!(
                keys.insert(entry.translation_key),
                "duplicate error key: {}",
                entry.translation_key
            );
        }
    }

    #[test]
    fn unknown_codes_fall_back_without_echoing_raw_input() {
        let error = NativeError::new("C:\\secret\\token.txt");
        assert_eq!(error.code, "NATIVE_UNKNOWN");
        assert!(!error.debug_message.contains("secret"));
    }
}
