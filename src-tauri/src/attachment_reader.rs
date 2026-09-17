use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

/// Reader copies live long enough for applications which open files
/// asynchronously, but are not a permanent attachment archive.
const READER_ENTRY_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const READER_QUOTA_BYTES: u64 = 256 * 1024 * 1024;

pub(crate) struct ReaderCache {
    root: PathBuf,
    maintenance: Mutex<()>,
}

impl ReaderCache {
    pub(crate) fn new(root: PathBuf) -> io::Result<Self> {
        fs::create_dir_all(&root)?;
        let cache = Self {
            root,
            maintenance: Mutex::new(()),
        };
        // Cleanup is best-effort at startup. In particular, Windows can refuse
        // to remove a file which is still open in another application. Such an
        // entry remains accounted for when the quota is next enforced.
        let _ = prune_reader_entries(
            &cache.root,
            SystemTime::now(),
            READER_ENTRY_TTL,
            READER_QUOTA_BYTES,
            0,
        );
        Ok(cache)
    }

    pub(crate) fn write(&self, filename: &str, bytes: &[u8]) -> io::Result<PathBuf> {
        // Defense in depth: don't trust the caller to have already stripped
        // path separators/`..`/absolute components out of `filename`. Only
        // accept it if it round-trips through `Path::file_name` unchanged.
        let name = Path::new(filename)
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| *name == filename)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid attachment filename"))?;

        let _guard = self
            .maintenance
            .lock()
            .map_err(|_| io::Error::other("attachment reader lock is poisoned"))?;
        prune_reader_entries(
            &self.root,
            SystemTime::now(),
            READER_ENTRY_TTL,
            READER_QUOTA_BYTES,
            bytes.len() as u64,
        )?;

        let directory = self.root.join(uuid::Uuid::new_v4().to_string());
        fs::create_dir(&directory)?;
        let path = directory.join(name);
        if let Err(error) = fs::write(&path, bytes) {
            let _ = fs::remove_dir_all(directory);
            return Err(error);
        }
        if let Err(error) = crate::attachment_security::quarantine(&path) {
            let _ = fs::remove_dir_all(directory);
            return Err(error);
        }
        Ok(path)
    }
}

#[derive(Debug)]
struct ReaderEntry {
    path: PathBuf,
    modified: SystemTime,
    size: u64,
}

fn reader_entries(root: &Path) -> io::Result<Vec<ReaderEntry>> {
    let mut entries = Vec::new();
    for result in fs::read_dir(root)? {
        let entry = result?;
        let file_type = entry.file_type()?;
        if !file_type.is_dir()
            || entry
                .file_name()
                .to_str()
                .and_then(|name| uuid::Uuid::parse_str(name).ok())
                .is_none()
        {
            continue;
        }
        let metadata = entry.metadata()?;
        entries.push(ReaderEntry {
            path: entry.path(),
            modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            size: directory_size(&entry.path())?,
        });
    }
    Ok(entries)
}

fn directory_size(path: &Path) -> io::Result<u64> {
    let mut size = 0_u64;
    for result in fs::read_dir(path)? {
        let entry = result?;
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            size = size.saturating_add(directory_size(&entry.path())?);
        } else if file_type.is_file() {
            size = size.saturating_add(entry.metadata()?.len());
        }
    }
    Ok(size)
}

fn prune_reader_entries(
    root: &Path,
    now: SystemTime,
    ttl: Duration,
    quota_bytes: u64,
    reserve_bytes: u64,
) -> io::Result<()> {
    if reserve_bytes > quota_bytes {
        return Err(io::Error::other(
            "attachment is larger than the reader cache quota",
        ));
    }

    let mut entries = reader_entries(root)?;
    entries.sort_by(|left, right| {
        left.modified
            .cmp(&right.modified)
            .then_with(|| left.path.cmp(&right.path))
    });

    let mut retained = Vec::new();
    for entry in entries {
        let expired = now
            .duration_since(entry.modified)
            .is_ok_and(|age| age >= ttl);
        if expired && fs::remove_dir_all(&entry.path).is_ok() {
            continue;
        }
        retained.push(entry);
    }

    let mut used_bytes = retained
        .iter()
        .fold(0_u64, |total, entry| total.saturating_add(entry.size));
    for entry in retained {
        if used_bytes.saturating_add(reserve_bytes) <= quota_bytes {
            break;
        }
        if fs::remove_dir_all(&entry.path).is_ok() {
            used_bytes = used_bytes.saturating_sub(entry.size);
        }
    }

    if used_bytes.saturating_add(reserve_bytes) > quota_bytes {
        return Err(io::Error::other(
            "unable to free enough space in the attachment reader cache",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_rejects_path_traversal_and_absolute_filenames() {
        let root = test_root("traversal");
        let cache = ReaderCache::new(root.clone()).unwrap();

        assert!(cache.write("../../etc/passwd", b"x").is_err());
        assert!(cache.write("nested/escape.txt", b"x").is_err());
        #[cfg(unix)]
        assert!(cache.write("/etc/passwd", b"x").is_err());

        assert!(cache.write("legitimate.txt", b"x").is_ok());
        let normalized =
            crate::attachment_security::normalize_filename("../invoice\u{202e}cod.ｅｘｅ");
        assert!(cache.write(&normalized, b"x").is_ok());
        fs::remove_dir_all(root).unwrap();
    }

    fn test_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("threestrands-reader-{name}-{}", uuid::Uuid::new_v4()))
    }

    fn add_entry(root: &Path, name: &str, bytes: usize) -> PathBuf {
        let path = root.join(name);
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("attachment"), vec![0; bytes]).unwrap();
        path
    }

    #[test]
    fn removes_entries_at_the_ttl_boundary() {
        let root = test_root("ttl");
        fs::create_dir_all(&root).unwrap();
        let entry = add_entry(&root, "00000000-0000-4000-8000-000000000001", 4);
        let modified = fs::metadata(&entry).unwrap().modified().unwrap();

        prune_reader_entries(&root, modified + READER_ENTRY_TTL, READER_ENTRY_TTL, 100, 0).unwrap();

        assert!(!entry.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn retains_entries_younger_than_the_ttl() {
        let root = test_root("fresh");
        fs::create_dir_all(&root).unwrap();
        let entry = add_entry(&root, "00000000-0000-4000-8000-000000000001", 4);
        let modified = fs::metadata(&entry).unwrap().modified().unwrap();

        prune_reader_entries(
            &root,
            modified + READER_ENTRY_TTL - Duration::from_secs(1),
            READER_ENTRY_TTL,
            100,
            0,
        )
        .unwrap();

        assert!(entry.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn evicts_oldest_entries_to_reserve_space_under_the_quota() {
        let root = test_root("quota");
        fs::create_dir_all(&root).unwrap();
        let first = add_entry(&root, "00000000-0000-4000-8000-000000000001", 6);
        let second = add_entry(&root, "00000000-0000-4000-8000-000000000002", 3);
        let now = fs::metadata(&second).unwrap().modified().unwrap();

        prune_reader_entries(&root, now, Duration::MAX, 10, 5).unwrap();

        assert!(!first.exists());
        assert!(second.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_a_single_attachment_larger_than_the_quota() {
        let root = test_root("oversize");
        fs::create_dir_all(&root).unwrap();

        let error =
            prune_reader_entries(&root, SystemTime::now(), Duration::MAX, 4, 5).unwrap_err();

        assert!(error.to_string().contains("larger"));
        fs::remove_dir_all(root).unwrap();
    }
}
