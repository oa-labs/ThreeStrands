//! Wrapper around macOS 26+'s native `SpeechAnalyzer` Swift shim.
//!
//! When built with a macOS 26+ SDK, `build.rs` compiles
//! `swift/speech_analyzer.swift` and defines `has_speech_analyzer_shim`. On
//! older SDKs or non-macOS targets, [`is_available`] returns `false` and
//! [`transcribe_file`] returns an error.

#[cfg(all(target_os = "macos", has_speech_analyzer_shim))]
mod mac {
    use serde::Deserialize;
    use std::ffi::{CStr, CString};
    use std::os::raw::{c_char, c_int};
    use std::path::Path;

    #[link(name = "threestrands_speech_shim", kind = "static")]
    extern "C" {
        fn threestrands_speech_analyzer_available() -> bool;
        fn threestrands_speech_analyzer_transcribe_file(
            path: *const c_char,
            language: *const c_char,
            prompt: *const c_char,
            out_json: *mut *mut c_char,
            out_err: *mut *mut c_char,
        ) -> c_int;
        fn threestrands_speech_analyzer_free_str(ptr: *mut c_char);
    }

    struct OwnedCStr(*mut c_char);
    impl OwnedCStr {
        fn into_string(self) -> String {
            if self.0.is_null() {
                return String::new();
            }
            // SAFETY: allocated by `strdup` inside the Swift shim and freed on Drop.
            unsafe { CStr::from_ptr(self.0) }
                .to_string_lossy()
                .into_owned()
        }
    }
    impl Drop for OwnedCStr {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: allocated by `strdup` inside the Swift shim.
                unsafe { threestrands_speech_analyzer_free_str(self.0) };
            }
        }
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct SegmentOut {
        start_ms: u64,
        end_ms: u64,
        text: String,
    }

    pub fn is_available() -> bool {
        // SAFETY: plain FFI availability check.
        unsafe { threestrands_speech_analyzer_available() }
    }

    pub fn transcribe_file(
        path: &Path,
        language: Option<&str>,
        prompt: Option<&str>,
    ) -> Result<Vec<(u64, u64, String)>, String> {
        let path_str = path
            .to_str()
            .ok_or_else(|| "The audio path isn't valid UTF-8.".to_string())?;
        let c_path = CString::new(path_str).map_err(|_| "Invalid audio path.".to_string())?;
        let c_lang = language
            .filter(|s| !s.is_empty())
            .map(CString::new)
            .transpose()
            .map_err(|_| "Invalid language code.".to_string())?;
        let c_prompt = prompt
            .filter(|s| !s.is_empty())
            .map(CString::new)
            .transpose()
            .map_err(|_| "Invalid initial prompt.".to_string())?;

        let mut out_json: *mut c_char = std::ptr::null_mut();
        let mut out_err: *mut c_char = std::ptr::null_mut();
        let status = unsafe {
            threestrands_speech_analyzer_transcribe_file(
                c_path.as_ptr(),
                c_lang.as_ref().map_or(std::ptr::null(), |s| s.as_ptr()),
                c_prompt.as_ref().map_or(std::ptr::null(), |s| s.as_ptr()),
                &mut out_json,
                &mut out_err,
            )
        };
        let json_guard = OwnedCStr(out_json);
        let err_guard = OwnedCStr(out_err);
        if status != 0 {
            let msg = err_guard.into_string();
            return Err(if msg.is_empty() {
                "Apple SpeechAnalyzer failed.".into()
            } else {
                msg
            });
        }
        let raw = json_guard.into_string();
        let items: Vec<SegmentOut> = serde_json::from_str(&raw)
            .map_err(|e| format!("Invalid SpeechAnalyzer output: {e}"))?;
        Ok(items
            .into_iter()
            .map(|item| (item.start_ms, item.end_ms.max(item.start_ms), item.text))
            .collect())
    }
}

#[cfg(not(all(target_os = "macos", has_speech_analyzer_shim)))]
mod stub {
    use std::path::Path;

    pub fn is_available() -> bool {
        false
    }

    pub fn transcribe_file(
        _path: &Path,
        _language: Option<&str>,
        _prompt: Option<&str>,
    ) -> Result<Vec<(u64, u64, String)>, String> {
        Err("Apple SpeechAnalyzer requires macOS 26 or later.".into())
    }
}

#[cfg(all(target_os = "macos", has_speech_analyzer_shim))]
pub use mac::{is_available, transcribe_file};
#[cfg(not(all(target_os = "macos", has_speech_analyzer_shim)))]
pub use stub::{is_available, transcribe_file};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn availability_matches_running_macos_version() {
        #[cfg(target_os = "macos")]
        {
            let (major, minor) = crate::recording::capture::macos::macos_version();
            if !crate::recording::policy::macos_has_speech_analyzer(major, minor) {
                assert!(!is_available());
            } else {
                // On macOS 26+, the shim links and reports a boolean without panicking.
                let _ = is_available();
            }
        }
        #[cfg(not(target_os = "macos"))]
        assert!(!is_available());
    }
}
