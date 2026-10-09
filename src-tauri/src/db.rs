//! Shared database connection and error infrastructure.
//!
//! Feature modules own their SQL and tests. Operations that coordinate features
//! (ingestion and settings transfer) keep a single caller-owned transaction.

use rusqlite::Connection;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Mutex, MutexGuard},
    thread::ThreadId,
};

mod accounts;
mod ai;
mod calendar_accounts;
pub(crate) mod contact_groups;
pub(crate) mod contact_management;
mod contact_suggestions;
pub(crate) mod contacts;
mod goals;
mod ingestion;
mod mail_sync;
mod maintenance;
mod messages;
mod mutations;
mod recovery;
mod search;
mod settings_transfer;
mod snippets;
mod split_inboxes;
mod tasks;
mod threads;
mod triage;

pub use mail_sync::SentBackfillProgress;
pub use mutations::PendingMutation;
pub use recovery::{open_with_recovery, OpenError, RecoveryOutcome};

#[cfg(test)]
pub(crate) mod test_support;

/// Errors produced by the local persistence layer. The command surface still
/// converts these to strings for backwards compatibility with the frontend,
/// but database and sync code can preserve the original category until that
/// boundary.
#[derive(Debug, thiserror::Error)]
pub enum DatabaseError {
    #[error("database connection lock was poisoned")]
    ConnectionPoisoned,
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("database validation failed: {0}")]
    Validation(String),
    #[error("database record not found: {0}")]
    NotFound(String),
    #[error("database serialization failed: {0}")]
    Serialization(String),
    #[error("{0}")]
    Message(String),
}

pub type DbResult<T> = Result<T, DatabaseError>;

impl From<String> for DatabaseError {
    fn from(message: String) -> Self {
        Self::Message(message)
    }
}

impl From<&str> for DatabaseError {
    fn from(message: &str) -> Self {
        Self::Message(message.to_string())
    }
}

/// Compatibility conversion for Tauri commands and older async service APIs
/// that still expose `Result<_, String>` to the frontend.
impl From<DatabaseError> for String {
    fn from(error: DatabaseError) -> Self {
        error.to_string()
    }
}

pub struct Database {
    connection: Mutex<Connection>,
    /// `None` only for the in-memory test database, which has no file to
    /// snapshot or checkpoint alongside.
    path: Option<PathBuf>,
    /// Threads currently applying an already-authenticated remote (or
    /// conflict-resolution) operation into local tables, with their nesting
    /// depth, so the shared materializer path used by both local commands
    /// and that projection does not re-enqueue the projected write as a new
    /// local event. Scoped per thread: a local command running on another
    /// thread while a projection is in progress must still be recorded. See
    /// `replicated_sync.rs`.
    pub(crate) replicated_sync_projecting: Mutex<HashMap<ThreadId, usize>>,
}

impl Database {
    pub(crate) fn connection(&self) -> DbResult<MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|_| DatabaseError::ConnectionPoisoned)
    }

    /// Runs one read or write operation while holding the connection guard.
    pub(crate) fn with_connection<R>(
        &self,
        work: impl FnOnce(&Connection) -> DbResult<R>,
    ) -> DbResult<R> {
        let connection = self.connection()?;
        work(&connection)
    }

    /// Runs an operation in a transaction and commits only when the operation
    /// succeeds. Dropping the transaction on an error rolls it back.
    pub(crate) fn with_transaction<R>(
        &self,
        work: impl FnOnce(&rusqlite::Transaction<'_>) -> DbResult<R>,
    ) -> DbResult<R> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        let result = work(&transaction)?;
        transaction.commit()?;
        Ok(result)
    }
}

fn normalize_sender(value: &str) -> (String, String) {
    let trimmed = value.trim();
    let candidate = match (trimmed.rfind('<'), trimmed.rfind('>')) {
        (Some(open), Some(close)) if close > open => &trimmed[open + 1..close],
        _ => trimmed,
    };
    let email = candidate
        .trim()
        .trim_matches(|character| character == '"' || character == '\'')
        .to_ascii_lowercase();
    let domain = email
        .rsplit_once('@')
        .map(|(_, domain)| domain.to_string())
        .unwrap_or_default();
    (email, domain)
}

fn decode_json(value: String) -> rusqlite::Result<Vec<String>> {
    serde_json::from_str(&value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            value.len(),
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

pub(super) fn display_error(error: impl std::fmt::Display) -> DatabaseError {
    DatabaseError::Message(error.to_string())
}

fn serialization_error(error: serde_json::Error) -> DatabaseError {
    DatabaseError::Serialization(error.to_string())
}

#[cfg(test)]
#[path = "db/tests/connection.rs"]
mod connection_tests;
