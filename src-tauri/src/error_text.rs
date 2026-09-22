//! Shared conversion for APIs that still report errors to the frontend as
//! plain strings. Database code should prefer the typed `DatabaseError`.

/// Renders any displayable error as the `String` error used by Tauri
/// commands and the string-returning service APIs.
pub(crate) fn display(error: impl std::fmt::Display) -> String {
    error.to_string()
}
