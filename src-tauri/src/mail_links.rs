//! Hands `mailto:` links to the composer. Links reach the native layer from
//! macOS (when ThreeStrands is the default mail app, including the link that
//! launched it), from webview navigation, and from mailto unsubscribe. Each is
//! queued here and announced to the webview, which drains the queue and
//! parses the link itself; the link is treated as untrusted text there.
//!
//! The queue exists because macOS can deliver a link before the webview has
//! loaded and started listening.

use std::sync::Mutex;

use tauri::{AppHandle, Emitter, Manager, Runtime, State};

use crate::limits::{MAX_MAIL_LINK_BYTES, MAX_PENDING_MAIL_LINKS};

/// Must match `MAIL_LINK_EVENT` in `src/mailtoLink.ts`.
const MAIL_LINK_EVENT: &str = "mail-link-received";

/// Links received but not yet taken by the webview.
#[derive(Default)]
pub struct MailLinkInbox(Mutex<Vec<String>>);

impl MailLinkInbox {
    /// Queues a mailto link. Returns false (and queues nothing) for any other
    /// scheme, an oversized link, or a full queue.
    fn push(&self, url: &str) -> bool {
        if !is_mailto(url) || url.len() > MAX_MAIL_LINK_BYTES {
            return false;
        }
        let mut pending = self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if pending.len() >= MAX_PENDING_MAIL_LINKS {
            return false;
        }
        pending.push(url.to_string());
        true
    }

    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()))
    }
}

pub fn is_mailto(url: &str) -> bool {
    url.get(..7).is_some_and(|scheme| scheme.eq_ignore_ascii_case("mailto:"))
}

/// Queues a mailto link for the composer, tells the webview, and brings the
/// main window forward. Anything that is not an acceptable mailto link is
/// dropped with a log line.
pub fn deliver<R: Runtime>(handle: &AppHandle<R>, url: &str) {
    let Some(inbox) = handle.try_state::<MailLinkInbox>() else {
        return;
    };
    if !inbox.push(url) {
        log::warn!("Ignored a mail link that was not an acceptable mailto URL");
        return;
    }
    let _ = handle.emit(MAIL_LINK_EVENT, ());
    if let Some(window) = handle.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

#[tauri::command]
pub fn take_pending_mail_links(inbox: State<'_, MailLinkInbox>) -> Vec<String> {
    inbox.take()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundle_declares_the_mailto_scheme() {
        let plist = include_str!("../Info.plist");
        assert!(plist.contains("<key>CFBundleURLSchemes</key>"));
        assert!(plist.contains("<string>mailto</string>"));
    }

    #[test]
    fn queues_only_mailto_links() {
        let inbox = MailLinkInbox::default();
        assert!(inbox.push("mailto:jane@example.com?subject=Hi"));
        assert!(inbox.push("MAILTO:jane@example.com"));
        for rejected in [
            "https://example.com/",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "mailt",
            "",
        ] {
            assert!(!inbox.push(rejected), "{rejected}");
        }
        assert_eq!(
            inbox.take(),
            vec!["mailto:jane@example.com?subject=Hi", "MAILTO:jane@example.com"]
        );
        assert!(inbox.take().is_empty());
    }

    #[test]
    fn enforces_the_link_size_limit() {
        let inbox = MailLinkInbox::default();
        let prefix = "mailto:a@example.com?body=";
        let exact = format!("{prefix}{}", "x".repeat(MAX_MAIL_LINK_BYTES - prefix.len()));
        assert!(inbox.push(&exact[..exact.len() - 1]));
        assert!(inbox.push(&exact));
        assert!(!inbox.push(&format!("{exact}x")));
        assert_eq!(inbox.take().len(), 2);
    }

    #[test]
    fn enforces_the_pending_queue_limit() {
        let inbox = MailLinkInbox::default();
        for index in 0..MAX_PENDING_MAIL_LINKS {
            assert!(inbox.push(&format!("mailto:person{index}@example.com")));
        }
        assert!(!inbox.push("mailto:overflow@example.com"));
        assert_eq!(inbox.take().len(), MAX_PENDING_MAIL_LINKS);
        assert!(inbox.push("mailto:after-drain@example.com"));
    }
}
