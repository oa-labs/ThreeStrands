//! Text handling shared across mail sources: the search query language
//! ([`search_query`]), which the local index and IMAP each render in their
//! own way; quoted-history detection for previews and search
//! ([`quoted_history`]); readable preview lines ([`preview_text`]); and
//! promises in the user's own sent mail ([`promise_text`]).
//!
//! Everything here is pure: no database, network, file system, or UI.

pub mod preview_text;
pub mod promise_text;
pub mod quoted_history;
pub mod search_query;
