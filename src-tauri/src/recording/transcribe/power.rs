//! Whether the Mac is on AC power. Transcribing during a recording waits
//! for Stop on battery.

/// The power source, behind a trait so tests can switch it.
pub trait PowerSource: Send + Sync {
    fn on_ac_power(&self) -> bool;
}

/// The real power source.
pub struct SystemPower;

#[cfg(target_os = "macos")]
mod iokit {
    use std::ffi::{c_char, c_void, CStr};

    pub type CFTypeRef = *const c_void;
    pub type CFStringRef = *const c_void;

    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        pub fn IOPSCopyPowerSourcesInfo() -> CFTypeRef;
        pub fn IOPSGetProvidingPowerSourceType(snapshot: CFTypeRef) -> CFStringRef;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        pub fn CFRelease(value: CFTypeRef);
        pub fn CFStringGetCString(
            string: CFStringRef,
            buffer: *mut c_char,
            size: isize,
            encoding: u32,
        ) -> u8;
    }

    const UTF8: u32 = 0x0800_0100;

    /// The providing power source's type: "AC Power", "Battery Power", or
    /// "UPS Power". `None` if IOKit can't say.
    pub fn providing_source() -> Option<String> {
        // SAFETY: IOPSCopyPowerSourcesInfo returns an owned snapshot (or
        // null), released below. IOPSGetProvidingPowerSourceType returns a
        // string the snapshot owns (Get rule), copied out before release.
        unsafe {
            let snapshot = IOPSCopyPowerSourcesInfo();
            if snapshot.is_null() {
                return None;
            }
            let kind = IOPSGetProvidingPowerSourceType(snapshot);
            let mut buffer = [0 as c_char; 64];
            let copied = !kind.is_null()
                && CFStringGetCString(kind, buffer.as_mut_ptr(), buffer.len() as isize, UTF8) != 0;
            let text = copied.then(|| {
                CStr::from_ptr(buffer.as_ptr())
                    .to_string_lossy()
                    .into_owned()
            });
            CFRelease(snapshot);
            text
        }
    }
}

/// Whether a providing-source name means mains power. A Mac without a
/// battery reports "AC Power"; anything unknown counts as AC, so a Mac whose
/// source can't be read still transcribes during recordings.
pub fn is_ac(source: Option<&str>) -> bool {
    !matches!(source, Some("Battery Power"))
}

impl PowerSource for SystemPower {
    #[cfg(target_os = "macos")]
    fn on_ac_power(&self) -> bool {
        is_ac(iokit::providing_source().as_deref())
    }

    #[cfg(not(target_os = "macos"))]
    fn on_ac_power(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_battery_power_counts_as_battery() {
        assert!(is_ac(Some("AC Power")));
        assert!(is_ac(Some("UPS Power")));
        assert!(is_ac(None));
        assert!(!is_ac(Some("Battery Power")));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn iokit_names_a_power_source() {
        let source = iokit::providing_source();
        assert!(
            matches!(
                source.as_deref(),
                Some("AC Power" | "Battery Power" | "UPS Power") | None
            ),
            "{source:?}"
        );
        let _ = SystemPower.on_ac_power();
    }
}
