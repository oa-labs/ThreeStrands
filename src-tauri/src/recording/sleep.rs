//! Stops any active recording cleanly when the Mac goes to sleep, so waking
//! a laptop never silently resumes microphone capture.

#[cfg(target_os = "macos")]
mod mac {
    use crate::recording::runtime::RecordingRuntime;
    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2_app_kit::{NSWorkspace, NSWorkspaceWillSleepNotification};
    use objc2_foundation::{NSNotification, NSObjectProtocol};
    use std::ptr::NonNull;
    use std::sync::Arc;

    /// Keeps the `NSWorkspaceWillSleepNotification` observer alive.
    pub struct SleepObserver {
        _token: Retained<ProtocolObject<dyn NSObjectProtocol>>,
    }

    // SAFETY: the observer token is only stored to keep the registration alive
    // for the lifetime of the application.
    unsafe impl Send for SleepObserver {}
    unsafe impl Sync for SleepObserver {}

    pub fn install(runtime: Arc<RecordingRuntime>) -> SleepObserver {
        let workspace = NSWorkspace::sharedWorkspace();
        let center = workspace.notificationCenter();
        let block = block2::RcBlock::new(move |_notification: NonNull<NSNotification>| {
            let runtime = runtime.clone();
            // Run off the notification callback so stopping capture never blocks AppKit.
            let _ = std::thread::Builder::new()
                .name("recording-sleep-stop".into())
                .spawn(move || runtime.stop_for_sleep());
        });
        // SAFETY: `NSWorkspaceWillSleepNotification` is a valid static notification name,
        // and `block` matches the expected `void (^)(NSNotification *)` signature.
        let token = unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(NSWorkspaceWillSleepNotification),
                None,
                None,
                &block,
            )
        };
        SleepObserver { _token: token }
    }
}

#[cfg(not(target_os = "macos"))]
mod non_mac {
    use crate::recording::runtime::RecordingRuntime;
    use std::sync::Arc;

    pub struct SleepObserver;

    pub fn install(_runtime: Arc<RecordingRuntime>) -> SleepObserver {
        SleepObserver
    }
}

#[cfg(target_os = "macos")]
pub use mac::{install, SleepObserver};
#[cfg(not(target_os = "macos"))]
pub use non_mac::{install, SleepObserver};
