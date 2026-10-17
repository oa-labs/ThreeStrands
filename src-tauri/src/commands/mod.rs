//! Tauri command handlers, one module per domain. `invoke_handler` in
//! `lib.rs` registers each one by its path here.

pub(crate) mod accounts;
pub(crate) mod ai;
pub(crate) mod attachments;
pub(crate) mod calendar;
pub(crate) mod contacts;
pub(crate) mod mail;
pub(crate) mod replicated_sync;
pub(crate) mod settings;
pub(crate) mod sync;
pub(crate) mod tasks;
