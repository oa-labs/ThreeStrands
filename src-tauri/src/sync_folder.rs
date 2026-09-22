//! The user-selected sync-folder transport adapter (Phase 4 of the
//! replicated-sync plan). Implements [`SyncTransport`] against an ordinary
//! directory — no provider API, no account, no OAuth.
//!
//! Corpus layout beneath the configured folder:
//!
//! ```text
//! threestrands-sync/
//!   format-v1
//!   objects/<cid[..2]>/<cid>.block
//!   heads/<hex device id>.head
//! ```
//!
//! A device id is already a random 16-byte value carrying no user content
//! (see `replicated_sync.rs`), so it doubles as the plan's "opaque device
//! tag" directly — no separate derivation needed. `put_object` doesn't
//! carry the caller's `object_kind`, so every object (an encrypted chunk or
//! an unencrypted chunk index) is stored the same way, sharded by CID; nothing
//! about the corpus layout depends on the distinction.
//!
//! Every write goes to a same-directory unpredictable temporary name,
//! fsyncs, then atomically renames to its final name, so a reader never
//! observes a valid filename with partial bytes. Every read verifies the
//! file's bytes hash to the filename that named it before returning them.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use rand::{rngs::OsRng, RngCore};
use tokio::io::AsyncWriteExt;

use threestrands_sync_envelope::{compute_cid, decode_signed_head, encode_signed_head, DeviceId, SignedDeviceHead};
use threestrands_sync_transport::{
    Cid, HeadLocator, ObjectLocator, ScanPage, SyncTransport, TransportCapabilities, TransportError,
    TransportHealth, TransportInstanceId,
};

const CORPUS_DIR_NAME: &str = "threestrands-sync";
const FORMAT_MARKER: &str = "format-v1";
/// Generous relative to any real envelope chunk (at most a few hundred KB;
/// see `crates/sync-envelope/src/limits.rs`) — this is a defensive ceiling
/// against a corrupted or hostile file, not a tuned protocol limit.
const MAX_OBJECT_BYTES: u64 = 8 * 1024 * 1024;
const SCAN_PAGE_SIZE: usize = 500;

pub struct SyncFolderTransport {
    instance_id: TransportInstanceId,
    /// The `threestrands-sync` directory itself, resolved (canonicalized)
    /// at construction so every later path is validated against it.
    root: PathBuf,
}

impl SyncFolderTransport {
    /// Opens (creating if necessary) a sync-folder corpus rooted at
    /// `selected_folder/threestrands-sync`. `selected_folder` must already
    /// exist — this never creates arbitrary ancestor directories, and never
    /// follows a symlink at that boundary.
    pub async fn open(instance_id: impl Into<String>, selected_folder: &Path) -> Result<Self, TransportError> {
        let metadata = tokio::fs::symlink_metadata(selected_folder).await.map_err(io_error)?;
        if metadata.is_symlink() {
            return Err(TransportError::Permanent(
                "the selected sync folder must not itself be a symlink".to_string(),
            ));
        }
        if !metadata.is_dir() {
            return Err(TransportError::Permanent("the selected sync folder does not exist".to_string()));
        }
        let root = selected_folder.join(CORPUS_DIR_NAME);
        tokio::fs::create_dir_all(root.join("objects")).await.map_err(io_error)?;
        tokio::fs::create_dir_all(root.join("heads")).await.map_err(io_error)?;
        let marker = root.join(FORMAT_MARKER);
        if tokio::fs::metadata(&marker).await.is_err() {
            atomic_write(&root, &marker, b"1").await?;
        }
        let root = tokio::fs::canonicalize(&root).await.map_err(io_error)?;
        Ok(Self {
            instance_id: TransportInstanceId(instance_id.into()),
            root,
        })
    }

    /// The exact corpus root this instance is configured against —
    /// `Database::materialize_entity`-adjacent Settings UI code displays
    /// this so "delete synchronized data" can show the user precisely what
    /// it is about to remove.
    pub fn corpus_root(&self) -> &Path {
        &self.root
    }

    fn object_path(&self, cid: &Cid) -> Result<PathBuf, TransportError> {
        let name = sanitize_cid(&cid.0)?;
        let shard = &name[..2];
        Ok(self.root.join("objects").join(shard).join(format!("{name}.block")))
    }

    fn head_path(&self, device_id: &DeviceId) -> PathBuf {
        let tag: String = device_id.as_bytes().iter().map(|byte| format!("{byte:02x}")).collect();
        self.root.join("heads").join(format!("{tag}.head"))
    }

    /// Removes every validated corpus file beneath this instance's root —
    /// "delete synchronized data": never follows symlinks, never touches
    /// anything outside `objects/`, `heads/`, or the format marker, and
    /// reports rather than swallows a partial failure.
    pub async fn delete_all_corpus_data(&self) -> Result<(), TransportError> {
        let mut errors = Vec::new();
        for sub in ["objects", "heads"] {
            if let Err(error) = remove_dir_contents(&self.root.join(sub)).await {
                errors.push(error.to_string());
            }
        }
        let marker = self.root.join(FORMAT_MARKER);
        let _ = tokio::fs::remove_file(&marker).await;
        if errors.is_empty() {
            Ok(())
        } else {
            Err(TransportError::Permanent(format!(
                "could not delete every corpus file: {}",
                errors.join("; ")
            )))
        }
    }

    /// Total bytes currently stored in this instance's corpus, for the
    /// Settings UI's storage estimate. Best-effort: an unreadable entry is
    /// skipped rather than failing the whole estimate.
    pub async fn corpus_size_bytes(&self) -> Result<u64, TransportError> {
        directory_size(&self.root).await
    }
}

async fn directory_size(dir: &Path) -> Result<u64, TransportError> {
    let mut total = 0u64;
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(io_error(error)),
    };
    while let Some(entry) = entries.next_entry().await.map_err(io_error)? {
        let Ok(file_type) = entry.file_type().await else { continue };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            total += Box::pin(directory_size(&entry.path())).await?;
        } else if let Ok(metadata) = entry.metadata().await {
            total += metadata.len();
        }
    }
    Ok(total)
}

#[async_trait]
impl SyncTransport for SyncFolderTransport {
    fn instance_id(&self) -> TransportInstanceId {
        self.instance_id.clone()
    }

    fn capabilities(&self) -> TransportCapabilities {
        TransportCapabilities {
            enumeration: true,
            incremental_cursor: true,
        }
    }

    async fn put_object(&self, cid: &Cid, bytes: &[u8]) -> Result<ObjectLocator, TransportError> {
        if bytes.len() as u64 > MAX_OBJECT_BYTES {
            return Err(TransportError::Permanent("object exceeds the folder adapter's size limit".to_string()));
        }
        let path = self.object_path(cid)?;
        let dir = path.parent().expect("object_path always has objects/<shard> as its parent");
        tokio::fs::create_dir_all(dir).await.map_err(io_error)?;
        if tokio::fs::symlink_metadata(&path).await.is_ok() {
            // put_object is idempotent by CID: the existing file already
            // has these exact bytes (or something is very wrong with the
            // corpus, which get_object's own hash check will catch).
            return Ok(ObjectLocator {
                cid: cid.clone(),
                remote_id: Some(path.display().to_string()),
            });
        }
        atomic_write(dir, &path, bytes).await?;
        Ok(ObjectLocator {
            cid: cid.clone(),
            remote_id: Some(path.display().to_string()),
        })
    }

    async fn get_object(&self, cid: &Cid) -> Result<Vec<u8>, TransportError> {
        let path = self.object_path(cid)?;
        let bytes = read_regular_file(&path).await?;
        if bytes.len() as u64 > MAX_OBJECT_BYTES {
            return Err(TransportError::Corruption("stored object exceeds the size limit".to_string()));
        }
        if compute_cid(&bytes) != cid.0 {
            return Err(TransportError::Corruption(
                "stored object bytes do not match its filename CID".to_string(),
            ));
        }
        Ok(bytes)
    }

    async fn publish_head(&self, head: &SignedDeviceHead) -> Result<HeadLocator, TransportError> {
        let path = self.head_path(&head.head.device_id);
        let dir = path.parent().expect("head_path always has heads/ as its parent");
        tokio::fs::create_dir_all(dir).await.map_err(io_error)?;
        let bytes = encode_signed_head(head).map_err(|error| TransportError::Permanent(error.to_string()))?;
        atomic_write(dir, &path, &bytes).await?;
        Ok(HeadLocator {
            device_id: head.head.device_id,
            remote_id: Some(path.display().to_string()),
        })
    }

    async fn resolve_heads(&self, known: &[HeadLocator]) -> Result<Vec<SignedDeviceHead>, TransportError> {
        let mut heads = Vec::new();
        for locator in known {
            let path = self.head_path(&locator.device_id);
            match read_regular_file(&path).await {
                Ok(bytes) => match decode_signed_head(&bytes) {
                    Ok(signed) => heads.push(signed),
                    // A malformed or partially hydrated head file stays
                    // pending rather than failing the whole resolution.
                    Err(_) => continue,
                },
                Err(TransportError::NotFound) => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(heads)
    }

    async fn scan(&self, cursor: Option<&str>) -> Result<Option<ScanPage>, TransportError> {
        let offset: usize = match cursor {
            None => 0,
            Some(cursor) => match cursor.parse() {
                Ok(offset) => offset,
                // An unrecognized cursor (from a different format version,
                // or a corpus that was reset) is reported as "no more
                // pages this way" — the caller falls back to head-based
                // discovery rather than silently resuming into the wrong
                // epoch of the corpus.
                Err(_) => return Ok(None),
            },
        };

        let mut cids = list_object_cids(&self.root.join("objects")).await?;
        cids.sort();
        cids.dedup();

        let end = (offset + SCAN_PAGE_SIZE).min(cids.len());
        let page = cids.get(offset..end).unwrap_or_default().to_vec();
        let next_cursor = if end < cids.len() { Some(end.to_string()) } else { None };
        Ok(Some(ScanPage {
            objects: page
                .into_iter()
                .map(|cid| ObjectLocator { remote_id: None, cid: Cid(cid) })
                .collect(),
            next_cursor,
        }))
    }

    async fn delete_object(&self, cid: &Cid) -> Result<(), TransportError> {
        let path = self.object_path(cid)?;
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(io_error(error)),
        }
    }

    async fn health(&self) -> Result<TransportHealth, TransportError> {
        match tokio::fs::metadata(&self.root).await {
            Ok(metadata) if metadata.is_dir() => Ok(TransportHealth::Healthy),
            _ => Ok(TransportHealth::Unavailable(
                "the configured folder is missing or inaccessible".to_string(),
            )),
        }
    }
}

/// Reads one regular file's bytes, refusing a symlink, a directory, or
/// anything else that isn't plain file data.
async fn read_regular_file(path: &Path) -> Result<Vec<u8>, TransportError> {
    let metadata = tokio::fs::symlink_metadata(path).await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            TransportError::NotFound
        } else {
            io_error(error)
        }
    })?;
    if !metadata.is_file() {
        // Includes symlinks: `symlink_metadata` never follows them, so a
        // symlink is reported as "not a file" here rather than resolved.
        return Err(TransportError::NotFound);
    }
    tokio::fs::read(path).await.map_err(io_error)
}

async fn list_object_cids(objects_dir: &Path) -> Result<Vec<String>, TransportError> {
    let mut cids = Vec::new();
    let mut shard_entries = match tokio::fs::read_dir(objects_dir).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(cids),
        Err(error) => return Err(io_error(error)),
    };
    while let Some(shard_entry) = shard_entries.next_entry().await.map_err(io_error)? {
        let Ok(file_type) = shard_entry.file_type().await else { continue };
        if !file_type.is_dir() {
            continue;
        }
        let mut file_entries = match tokio::fs::read_dir(shard_entry.path()).await {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        while let Some(file_entry) = file_entries.next_entry().await.map_err(io_error)? {
            let Ok(file_type) = file_entry.file_type().await else { continue };
            if !file_type.is_file() {
                // Ignore symlinks, directories, and any other special file.
                continue;
            }
            let name = file_entry.file_name();
            let name = name.to_string_lossy();
            if let Some(cid) = name.strip_suffix(".block") {
                if sanitize_cid(cid).is_ok() {
                    cids.push(cid.to_string());
                }
            }
        }
    }
    Ok(cids)
}

async fn remove_dir_contents(dir: &Path) -> std::io::Result<()> {
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    while let Some(entry) = entries.next_entry().await? {
        let file_type = entry.file_type().await?;
        if file_type.is_symlink() {
            tokio::fs::remove_file(entry.path()).await?;
        } else if file_type.is_dir() {
            Box::pin(remove_dir_contents(&entry.path())).await?;
            tokio::fs::remove_dir(entry.path()).await?;
        } else {
            tokio::fs::remove_file(entry.path()).await?;
        }
    }
    Ok(())
}

/// Writes to a same-directory unpredictable temporary name, fsyncs, then
/// atomically renames into place — a reader can never observe `final_path`
/// with partial bytes.
async fn atomic_write(dir: &Path, final_path: &Path, bytes: &[u8]) -> Result<(), TransportError> {
    let suffix = OsRng.next_u64();
    let temp_path = dir.join(format!(".tmp-{suffix:016x}-{}", std::process::id()));
    {
        let mut file = tokio::fs::File::create(&temp_path).await.map_err(io_error)?;
        file.write_all(bytes).await.map_err(io_error)?;
        file.sync_all().await.map_err(io_error)?;
    }
    if let Err(error) = tokio::fs::rename(&temp_path, final_path).await {
        let _ = tokio::fs::remove_file(&temp_path).await;
        return Err(io_error(error));
    }
    Ok(())
}

/// CIDv1 strings are plain lowercase base32 (RFC4648) after their `b`
/// multibase prefix: alphanumeric only. Rejecting anything else here closes
/// off path traversal, absolute paths, and null-byte tricks before a CID
/// ever reaches a filesystem call.
fn sanitize_cid(cid: &str) -> Result<&str, TransportError> {
    if cid.len() < 8 || cid.len() > 256 || !cid.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(TransportError::Permanent("invalid content identifier".to_string()));
    }
    Ok(cid)
}

fn io_error(error: std::io::Error) -> TransportError {
    match error.kind() {
        std::io::ErrorKind::NotFound => TransportError::NotFound,
        std::io::ErrorKind::PermissionDenied => TransportError::Permanent(error.to_string()),
        _ => TransportError::Transient(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::OsRng as TestOsRng;
    use threestrands_sync_envelope::{sign_device_head, DeviceHead, SigningKey};
    use threestrands_sync_transport::conformance;
    use uuid::Uuid;

    /// A real temporary directory, removed on drop even if the test
    /// panics.
    struct TempFolder {
        path: PathBuf,
    }

    impl TempFolder {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("threestrands-sync-folder-test-{}", Uuid::new_v4()));
            std::fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for TempFolder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    async fn open_transport(folder: &TempFolder, instance_id: &str) -> SyncFolderTransport {
        SyncFolderTransport::open(instance_id, &folder.path).await.unwrap()
    }

    #[tokio::test]
    async fn passes_the_shared_conformance_suite() {
        let folder_a = TempFolder::new();
        let folder_b = TempFolder::new();
        let a = open_transport(&folder_a, "a").await;
        let b = open_transport(&folder_b, "b").await;
        conformance::run_all(&a, &b).await;
    }

    #[tokio::test]
    async fn a_corpus_copied_byte_for_byte_bootstraps_identically() {
        let source_folder = TempFolder::new();
        let source = open_transport(&source_folder, "source").await;
        let bytes = b"hello corpus".to_vec();
        let cid = Cid::for_bytes(&bytes);
        source.put_object(&cid, &bytes).await.unwrap();

        let signing_key = SigningKey::generate(&mut TestOsRng);
        let head = DeviceHead {
            sync_space_id: b"space".to_vec(),
            device_id: DeviceId::from_bytes([3u8; 16]),
            epoch: 0,
            contiguous_sequence: 1,
            latest_event_cid: Some(cid.0.clone()),
        };
        let signed = sign_device_head(&signing_key, head).unwrap();
        source.publish_head(&signed).await.unwrap();

        // Copy the corpus directory byte-for-byte to a second folder.
        let target_folder = TempFolder::new();
        copy_dir_recursive(&source_folder.path, &target_folder.path).await;
        let target = open_transport(&target_folder, "target").await;

        assert_eq!(target.get_object(&cid).await.unwrap(), bytes);
        let resolved = target
            .resolve_heads(&[HeadLocator {
                device_id: DeviceId::from_bytes([3u8; 16]),
                remote_id: None,
            }])
            .await
            .unwrap();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0], signed);
    }

    async fn copy_dir_recursive(from: &Path, to: &Path) {
        let mut entries = tokio::fs::read_dir(from).await.unwrap();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            let file_type = entry.file_type().await.unwrap();
            let dest = to.join(entry.file_name());
            if file_type.is_dir() {
                tokio::fs::create_dir_all(&dest).await.unwrap();
                Box::pin(copy_dir_recursive(&entry.path(), &dest)).await;
            } else {
                tokio::fs::copy(entry.path(), &dest).await.unwrap();
            }
        }
    }

    #[tokio::test]
    async fn a_corrupted_object_is_reported_and_never_silently_accepted() {
        let folder = TempFolder::new();
        let transport = open_transport(&folder, "a").await;
        let bytes = b"authentic bytes".to_vec();
        let cid = Cid::for_bytes(&bytes);
        transport.put_object(&cid, &bytes).await.unwrap();

        // Corrupt the stored file directly, as a faulty disk or a
        // mid-write crash outside our own atomic-write path might.
        let path = transport.object_path(&cid).unwrap();
        tokio::fs::write(&path, b"tampered bytes!!").await.unwrap();

        let result = transport.get_object(&cid).await;
        assert_eq!(result, Err(TransportError::Corruption(
            "stored object bytes do not match its filename CID".to_string()
        )));
    }

    #[tokio::test]
    async fn a_symlinked_object_file_is_never_read() {
        let folder = TempFolder::new();
        let transport = open_transport(&folder, "a").await;
        let bytes = b"real target".to_vec();
        let cid = Cid::for_bytes(&bytes);
        let real_path = folder.path.join("real-target.bin");
        tokio::fs::write(&real_path, &bytes).await.unwrap();

        let path = transport.object_path(&cid).unwrap();
        tokio::fs::create_dir_all(path.parent().unwrap()).await.unwrap();
        #[cfg(unix)]
        tokio::fs::symlink(&real_path, &path).await.unwrap();
        #[cfg(windows)]
        tokio::fs::symlink_file(&real_path, &path).await.unwrap();

        assert_eq!(transport.get_object(&cid).await, Err(TransportError::NotFound));
    }

    #[tokio::test]
    async fn path_traversal_and_malformed_cids_fail_closed() {
        let folder = TempFolder::new();
        let transport = open_transport(&folder, "a").await;
        for malformed in ["../../etc/passwd", "..", "", "has spaces", "has/slash", "a"] {
            let cid = Cid(malformed.to_string());
            assert!(transport.put_object(&cid, b"x").await.is_err());
            assert!(transport.get_object(&cid).await.is_err() || matches!(
                transport.get_object(&cid).await,
                Err(TransportError::NotFound)
            ));
        }
    }

    #[tokio::test]
    async fn oversized_objects_are_rejected() {
        let folder = TempFolder::new();
        let transport = open_transport(&folder, "a").await;
        let bytes = vec![0u8; (MAX_OBJECT_BYTES + 1) as usize];
        let cid = Cid::for_bytes(&bytes);
        assert!(matches!(
            transport.put_object(&cid, &bytes).await,
            Err(TransportError::Permanent(_))
        ));
    }

    #[tokio::test]
    async fn opening_refuses_a_symlinked_selection() {
        let real_folder = TempFolder::new();
        let link_path = std::env::temp_dir().join(format!("threestrands-sync-folder-symlink-{}", Uuid::new_v4()));
        #[cfg(unix)]
        tokio::fs::symlink(&real_folder.path, &link_path).await.unwrap();
        #[cfg(unix)]
        {
            let result = SyncFolderTransport::open("a", &link_path).await;
            assert!(result.is_err());
            let _ = tokio::fs::remove_file(&link_path).await;
        }
    }

    #[tokio::test]
    async fn delete_all_corpus_data_removes_only_the_corpus() {
        let folder = TempFolder::new();
        let transport = open_transport(&folder, "a").await;
        let bytes = b"to be deleted".to_vec();
        let cid = Cid::for_bytes(&bytes);
        transport.put_object(&cid, &bytes).await.unwrap();

        // An unrelated file the user has elsewhere in the same selected
        // folder must survive.
        let sibling = folder.path.join("unrelated-user-file.txt");
        tokio::fs::write(&sibling, b"do not touch").await.unwrap();

        transport.delete_all_corpus_data().await.unwrap();

        assert_eq!(transport.get_object(&cid).await, Err(TransportError::NotFound));
        assert!(tokio::fs::metadata(&sibling).await.is_ok());
    }

    #[tokio::test]
    async fn scan_paginates_and_a_stale_cursor_is_reported_as_unrecognized() {
        let folder = TempFolder::new();
        let transport = open_transport(&folder, "a").await;
        for index in 0..3 {
            let bytes = format!("object {index}").into_bytes();
            let cid = Cid::for_bytes(&bytes);
            transport.put_object(&cid, &bytes).await.unwrap();
        }
        let page = transport.scan(None).await.unwrap().unwrap();
        assert_eq!(page.objects.len(), 3);
        assert!(page.next_cursor.is_none());

        assert_eq!(transport.scan(Some("not-a-number")).await.unwrap(), None);
    }

    #[tokio::test]
    async fn corpus_size_reflects_stored_objects() {
        let folder = TempFolder::new();
        let transport = open_transport(&folder, "a").await;
        // A freshly opened corpus already has its one-byte `format-v1`
        // marker; assert growth relative to that rather than assuming zero.
        let before = transport.corpus_size_bytes().await.unwrap();
        let bytes = vec![7u8; 1024];
        let cid = Cid::for_bytes(&bytes);
        transport.put_object(&cid, &bytes).await.unwrap();
        assert!(transport.corpus_size_bytes().await.unwrap() >= before + 1024);
    }
}
