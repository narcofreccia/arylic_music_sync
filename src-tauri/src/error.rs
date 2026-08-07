//! Unified error type for command results. Serializes to a `{ code, message }`
//! envelope so the frontend can branch on the stable `code` string instead of
//! pattern-matching human-readable (and translatable) messages.

use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// Persisted config could not be read/written (tauri-plugin-store).
    #[error("storage error: {0}")]
    Store(String),

    /// Wrong credentials, missing profile, profile already exists — anything
    /// the user can fix by typing something different.
    #[error("{0}")]
    Auth(String),

    /// Too many failed logins; `secs` is the remaining cool-down.
    #[error("Too many failed attempts. Try again in {secs}s.")]
    LockedOut { secs: u64 },

    /// Malformed input caught before it reaches the hashing/storage layer.
    #[error("{0}")]
    InvalidInput(String),

    /// A device was unreachable, timed out, or answered something we can't use.
    /// Routine on a LAN (NFR-3), so the UI shows it inline rather than as a fault.
    #[error("{0}")]
    Device(String),

    /// No device with that UUID is in the saved list — a stale UI reference.
    #[error("{0}")]
    NotFound(String),

    /// A bug or an environment failure — never actionable by the user.
    #[error("{0}")]
    Internal(String),
}

impl AppError {
    /// The stable machine code the frontend switches on (mirrors `Serialize`).
    pub fn code(&self) -> &str {
        match self {
            AppError::Store(_) => "store_error",
            AppError::Auth(_) => "auth_error",
            AppError::LockedOut { .. } => "locked_out",
            AppError::InvalidInput(_) => "invalid_input",
            AppError::Device(_) => "device_error",
            AppError::NotFound(_) => "not_found",
            AppError::Internal(_) => "internal_error",
        }
    }
}

impl From<tauri_plugin_store::Error> for AppError {
    fn from(e: tauri_plugin_store::Error) -> Self {
        AppError::Store(e.to_string())
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        AppError::Store(format!("config serialization failed: {e}"))
    }
}

impl Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut s = serializer.serialize_struct("AppError", 2)?;
        s.serialize_field("code", self.code())?;
        s.serialize_field("message", &self.to_string())?;
        s.end()
    }
}

pub type AppResult<T> = Result<T, AppError>;
