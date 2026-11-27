//! Owner-only permissions and backup exclusion for local recording audio and
//! model files.
//!
//! Raw audio of other people is the most sensitive local data the app holds:
//! audio folders and files are created `0700`/`0600` on Unix, and marked with
//! `NSURLIsExcludedFromBackupKey` on macOS (along with `recording-models/`) so
//! Time Machine and third-party backup tools never archive deleted audio or
//! downloaded models off-device.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;

/// Creates `path` (and parents), narrows its permissions to `0700` on Unix,
/// and marks it excluded from backups on macOS.
pub fn ensure_private_dir(path: &Path) -> io::Result<()> {
    std::fs::create_dir_all(path)?;
    restrict_dir_to_owner(path)?;
    exclude_from_backup(path);
    Ok(())
}

/// Narrows an existing directory to `0700` on Unix.
pub fn restrict_dir_to_owner(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Narrows an existing file to `0600` on Unix.
pub fn restrict_file_to_owner(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Creates a brand-new file with `0600` permissions on Unix.
pub fn create_private_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// Opens (or creates) an append-only file with `0600` permissions on Unix.
pub fn open_append_private_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// Best-effort: marks `path` with `NSURLIsExcludedFromBackupKey` on macOS.
pub fn exclude_from_backup(path: &Path) {
    #[cfg(target_os = "macos")]
    {
        use objc2::runtime::AnyObject;
        use objc2_foundation::{NSNumber, NSString, NSURLIsExcludedFromBackupKey, NSURL};

        let Some(raw) = path.to_str() else {
            return;
        };
        let url = NSURL::fileURLWithPath(&NSString::from_str(raw));
        let yes = NSNumber::new_bool(true);
        // SAFETY: `NSNumber` is a valid `AnyObject` value for `NSURLIsExcludedFromBackupKey`.
        let value: &AnyObject =
            unsafe { &*(objc2::rc::Retained::as_ptr(&yes) as *const AnyObject) };
        // SAFETY: `NSURLIsExcludedFromBackupKey` is a static `NSURLResourceKey`.
        let key = unsafe { NSURLIsExcludedFromBackupKey };
        if let Err(error) = unsafe { url.setResourceValue_forKey_error(Some(value), key) } {
            log::warn!(
                target: "recording",
                "excluding {} from backup: {:?}",
                path.display(),
                error
            );
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = path;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::test_support::TempDir;

    #[cfg(unix)]
    #[test]
    fn directories_and_files_get_owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new();
        let sub = dir.path().join("private");
        ensure_private_dir(&sub).unwrap();
        let mode = std::fs::metadata(&sub).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);

        let file_path = sub.join("mic-0001.wav");
        create_private_file(&file_path).unwrap();
        let fmode = std::fs::metadata(&file_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(fmode, 0o600);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_marks_directory_excluded_from_backup() {
        let dir = TempDir::new();
        let sub = dir.path().join("recordings");
        ensure_private_dir(&sub).unwrap();
        let value = xattr::get(&sub, "com.apple.metadata:com_apple_backup_excludeItem")
            .unwrap()
            .expect("backup exclusion extended attribute should be set");
        assert!(!value.is_empty());
    }
}
