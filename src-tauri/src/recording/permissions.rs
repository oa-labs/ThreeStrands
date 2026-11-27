//! Asking for the permissions recording needs, when the user turns
//! **Record Meetings** on, so macOS doesn't interrupt a meeting to ask.
//!
//! The microphone goes through AVFoundation's `AVCaptureDevice` (called with
//! raw `msg_send!`, the framework linked here, so no new crate). System audio
//! has no public status check: creating a process tap once is what makes
//! macOS show its prompt, so on 14.4 and later a tap is created and removed.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MicrophoneAccess {
    Granted,
    Denied,
    /// Blocked by a device policy; the user can't change it.
    Restricted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingPermissions {
    pub microphone: MicrophoneAccess,
}

/// `AVAuthorizationStatus` to what the app reports. `None` means not yet
/// determined: the app must ask.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn microphone_access(status: isize) -> Option<MicrophoneAccess> {
    match status {
        3 => Some(MicrophoneAccess::Granted),
        2 => Some(MicrophoneAccess::Denied),
        1 => Some(MicrophoneAccess::Restricted),
        _ => None,
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use super::{microphone_access, MicrophoneAccess};
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, Bool};
    use objc2_foundation::NSString;
    use std::time::Duration;

    #[link(name = "AVFoundation", kind = "framework")]
    extern "C" {
        static AVMediaTypeAudio: &'static NSString;
    }

    /// How long to wait for the user to answer the microphone prompt.
    const ANSWER_WAIT: Duration = Duration::from_secs(10 * 60);

    fn capture_device_class() -> Option<&'static AnyClass> {
        AnyClass::get(c"AVCaptureDevice")
    }

    #[cfg(test)]
    pub fn capture_device_class_for_tests() -> bool {
        capture_device_class().is_some()
    }

    /// The microphone permission, asking first if macOS hasn't yet. Blocks
    /// until the user answers; call it off the main thread.
    pub fn microphone() -> MicrophoneAccess {
        let Some(class) = capture_device_class() else {
            return MicrophoneAccess::Denied;
        };
        // SAFETY: a class method taking an AVMediaType and returning
        // AVAuthorizationStatus (NSInteger).
        let status: isize =
            unsafe { msg_send![class, authorizationStatusForMediaType: AVMediaTypeAudio] };
        if let Some(access) = microphone_access(status) {
            return access;
        }
        let (sender, receiver) = std::sync::mpsc::channel();
        let handler = block2::RcBlock::new(move |granted: Bool| {
            let _ = sender.send(granted.as_bool());
        });
        // SAFETY: the completion handler is a `void (^)(BOOL)` block, which
        // AVFoundation copies before returning.
        unsafe {
            let _: () = msg_send![
                class,
                requestAccessForMediaType: AVMediaTypeAudio,
                completionHandler: &*handler
            ];
        }
        match receiver.recv_timeout(ANSWER_WAIT) {
            Ok(true) => MicrophoneAccess::Granted,
            _ => MicrophoneAccess::Denied,
        }
    }
}

/// Asks for every permission recording needs on this device and reports the
/// microphone's. Blocks while a prompt is open.
pub fn request(system_audio_available: bool) -> RecordingPermissions {
    #[cfg(target_os = "macos")]
    {
        let microphone = mac::microphone();
        if system_audio_available {
            if let Err(error) = super::capture::macos::probe_system_audio_permission() {
                log::warn!(target: "recording", "asking for system audio permission: {error}");
            }
        }
        RecordingPermissions { microphone }
    }
    #[cfg(not(target_os = "macos"))]
    {
        // No operating system prompt to show here; recording itself is not
        // available on these platforms yet.
        let _ = system_audio_available;
        RecordingPermissions {
            microphone: MicrophoneAccess::Granted,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorization_statuses_map_to_what_the_app_reports() {
        assert_eq!(microphone_access(0), None, "not determined: ask");
        assert_eq!(microphone_access(1), Some(MicrophoneAccess::Restricted));
        assert_eq!(microphone_access(2), Some(MicrophoneAccess::Denied));
        assert_eq!(microphone_access(3), Some(MicrophoneAccess::Granted));
        assert_eq!(
            serde_json::to_value(RecordingPermissions {
                microphone: MicrophoneAccess::Restricted
            })
            .unwrap(),
            serde_json::json!({ "microphone": "restricted" })
        );
    }

    #[test]
    fn the_bundle_declares_what_recording_needs() {
        let plist = include_str!("../../Info.plist");
        for key in [
            "NSMicrophoneUsageDescription",
            "NSAudioCaptureUsageDescription",
        ] {
            let at = plist
                .find(&format!("<key>{key}</key>"))
                .unwrap_or_else(|| panic!("{key}"));
            let value = plist[at..].split("<string>").nth(1).unwrap();
            assert!(value.split("</string>").next().unwrap().len() > 20, "{key}");
        }
        let entitlements = include_str!("../../Entitlements.plist");
        assert!(
            entitlements.contains("<key>com.apple.security.device.audio-input</key>\n\t<true/>")
        );
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap();
        let mac = &config["bundle"]["macOS"];
        assert_eq!(mac["entitlements"], "./Entitlements.plist");
        assert_eq!(mac["hardenedRuntime"], true);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn avfoundation_is_linked() {
        assert!(mac::capture_device_class_for_tests());
    }
}
