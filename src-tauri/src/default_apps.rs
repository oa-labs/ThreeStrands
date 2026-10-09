//! Lets the user make ThreeStrands the macOS default for email links and
//! calendar invitations (Settings → Default Apps). macOS asks the user to
//! confirm each change, so the app can request the role but never take it
//! silently. Info.plist declares both roles; see mail_links.rs and
//! calendar_files.rs for what happens when a link or file arrives.

use serde::{Deserialize, Serialize};

/// The uniform type identifier macOS gives `.ics` files.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const ICS_CONTENT_TYPE: &str = "com.apple.ical.ics";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DefaultAppRole {
    /// `mailto:` links.
    Mail,
    /// `.ics` calendar invitations.
    Calendar,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DefaultAppStatus {
    /// False on other platforms and in unbundled development builds, which
    /// macOS cannot register as a handler.
    pub supported: bool,
    pub mail: bool,
    pub calendar: bool,
}

const UNSUPPORTED: DefaultAppStatus = DefaultAppStatus {
    supported: false,
    mail: false,
    calendar: false,
};

#[tauri::command]
pub fn default_app_status() -> DefaultAppStatus {
    platform::status().unwrap_or(UNSUPPORTED)
}

#[tauri::command]
pub async fn make_default_app(role: DefaultAppRole) -> Result<DefaultAppStatus, String> {
    platform::request(role).await?;
    Ok(default_app_status())
}

#[cfg(target_os = "macos")]
mod platform {
    use std::sync::Mutex;

    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::{NSBundle, NSError, NSString, NSURL};
    use objc2_uniform_type_identifiers::UTType;

    use super::{DefaultAppRole, DefaultAppStatus, ICS_CONTENT_TYPE};

    /// This app's bundle and identifier, or None when running outside a
    /// `.app` (as `tauri dev` does), where there is nothing to register.
    fn own_bundle() -> Option<(Retained<NSURL>, Retained<NSString>)> {
        let bundle = NSBundle::mainBundle();
        let identifier = bundle.bundleIdentifier()?;
        let url = bundle.bundleURL();
        url.path()?.to_string().ends_with(".app").then_some((url, identifier))
    }

    /// Compares by bundle identifier so a second copy of ThreeStrands (for
    /// example one still on a disk image) still counts as ThreeStrands.
    fn is_own(app: Option<Retained<NSURL>>, identifier: &NSString) -> bool {
        app.and_then(|url| NSBundle::bundleWithURL(&url))
            .and_then(|bundle| bundle.bundleIdentifier())
            .is_some_and(|other| other.to_string() == identifier.to_string())
    }

    fn ics_type() -> Option<Retained<UTType>> {
        UTType::typeWithIdentifier(&NSString::from_str(ICS_CONTENT_TYPE))
    }

    pub fn status() -> Option<DefaultAppStatus> {
        let (_, identifier) = own_bundle()?;
        let workspace = NSWorkspace::sharedWorkspace();
        let mail = NSURL::URLWithString(&NSString::from_str("mailto:"))
            .and_then(|probe| workspace.URLForApplicationToOpenURL(&probe));
        let calendar = ics_type().and_then(|ics| workspace.URLForApplicationToOpenContentType(&ics));
        Some(DefaultAppStatus {
            supported: true,
            mail: is_own(mail, &identifier),
            calendar: is_own(calendar, &identifier),
        })
    }

    pub async fn request(role: DefaultAppRole) -> Result<(), String> {
        let (sender, receiver) = tokio::sync::oneshot::channel::<Result<(), String>>();
        // The Objective-C objects are not Send, so they live only in this
        // block and are released before awaiting the user's answer.
        {
            let (app, _) = own_bundle().ok_or_else(|| {
                "Default apps can only be set from an installed copy of ThreeStrands.".to_string()
            })?;
            let sender = Mutex::new(Some(sender));
            let completion = RcBlock::new(move |error: *mut NSError| {
                // SAFETY: AppKit passes either null or a valid NSError that
                // lives for the duration of the completion handler.
                let result = match unsafe { error.as_ref() } {
                    None => Ok(()),
                    Some(error) => Err(error.localizedDescription().to_string()),
                };
                if let Some(sender) = sender.lock().ok().and_then(|mut sender| sender.take()) {
                    let _ = sender.send(result);
                }
            });
            let workspace = NSWorkspace::sharedWorkspace();
            match role {
                DefaultAppRole::Mail => workspace
                    .setDefaultApplicationAtURL_toOpenURLsWithScheme_completionHandler(
                        &app,
                        &NSString::from_str("mailto"),
                        Some(&completion),
                    ),
                DefaultAppRole::Calendar => {
                    let ics = ics_type().ok_or("macOS does not recognize calendar files")?;
                    workspace.setDefaultApplicationAtURL_toOpenContentType_completionHandler(
                        &app,
                        &ics,
                        Some(&completion),
                    );
                }
            }
        }
        receiver
            .await
            .map_err(|_| "macOS did not finish changing the default app".to_string())?
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::{DefaultAppRole, DefaultAppStatus};

    pub fn status() -> Option<DefaultAppStatus> {
        None
    }

    pub async fn request(_role: DefaultAppRole) -> Result<(), String> {
        Err("Default apps can only be set on macOS.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_use_the_webview_names() {
        assert_eq!(serde_json::from_str::<DefaultAppRole>("\"mail\"").unwrap(), DefaultAppRole::Mail);
        assert_eq!(serde_json::from_str::<DefaultAppRole>("\"calendar\"").unwrap(), DefaultAppRole::Calendar);
        assert!(serde_json::from_str::<DefaultAppRole>("\"browser\"").is_err());
    }

    #[test]
    fn an_unbundled_build_reports_no_support() {
        // Tests run as a bare binary, never from a `.app`.
        assert_eq!(default_app_status(), UNSUPPORTED);
    }

    #[test]
    fn the_bundle_declares_the_calendar_document_type() {
        let plist = include_str!("../Info.plist");
        assert!(plist.contains("<key>CFBundleDocumentTypes</key>"));
        assert!(plist.contains(&format!("<string>{ICS_CONTENT_TYPE}</string>")));
    }
}
