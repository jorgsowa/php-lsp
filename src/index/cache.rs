//! Persistent on-disk cache for Phase K.
//!
//! The cache stores a serialized `FileIndex` per PHP file, keyed on
//! `(uri, content)`.  On a warm start `scan_workspace` reads the cached index
//! instead of parsing the file, shrinking cold-start I/O from O(parse) to
//! O(read + bincode-decode) — roughly 10–50× faster per file.
//!
//! ## Layout
//!
//! ```text
//! ~/.cache/php-lsp/<schema-version>/<workspace-hash>/<uri-hash>.bin
//! ```
//!
//! - `<schema-version>` — `php-lsp` crate version; bumping it rotates the
//!   entire cache so old entries are never decoded against a newer schema.
//!   The previous version's directory is pruned by the next `WorkspaceCache::new`
//!   call rather than left to accumulate release over release.
//! - `<workspace-hash>` — blake3 of the canonicalized absolute path of the
//!   first workspace root, truncated to 16 hex chars. Two separate projects
//!   get isolated caches; two checkouts of the same project at the same
//!   absolute path share one. Holds a `workspace.json` recording the root
//!   path and last-used time, so the background pruner can drop caches for
//!   roots that no longer exist or haven't been opened in
//!   [`WORKSPACE_MAX_IDLE`].
//! - `<uri-hash>` — blake3 of the URI, truncated to 32 hex chars. One file
//!   per URI, not per `(uri, content)` pair: a content hash is stored inside
//!   the entry and checked on read, so re-indexing an edited file overwrites
//!   its existing slot in place instead of leaving the previous revision's
//!   entry to rot on disk.
//!
//! `<workspace-hash>` also holds a `session/` subdirectory: mir's own
//! `AnalysisCache`, nested here (not under mir's default `.mir-cache/`) so
//! both caches share schema/workspace rotation and cleanup. `size_bytes`
//! recurses into it for the size cap.
//!
//! ## Format
//!
//! `bincode` v2 (binary, fast, schema-stable via serde derives on
//! `FileIndex` et al), prefixed with the entry's content hash so a stale
//! revision reads back as a miss. Files are written atomically via a
//! temp-file rename to avoid half-written entries on an interrupted
//! shutdown.
//!
//! ## Invalidation
//!
//! Rotating the schema version invalidates everything; rotating the content
//! invalidates just that entry's slot (overwritten on next write). The size
//! cap in [`WorkspaceCache::new`] is a backstop for workspaces that outgrow
//! it, not the primary cleanup mechanism.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Serialize, de::DeserializeOwned};

/// Identifies a single cache entry. Opaque — callers produce it via
/// [`WorkspaceCache::key_for`] and pass it straight back to read/write.
///
/// `uri_hash` is the on-disk filename, so every revision of a given URI
/// lands in the same slot. `content_hash` is stored inside the entry and
/// checked on read, so an edit invalidates the slot without orphaning a
/// separate file for the old content.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CacheKey {
    uri_hash: String,
    content_hash: String,
}

impl CacheKey {
    fn as_filename(&self) -> &str {
        &self.uri_hash
    }
}

/// Handle to the cache directory for a single workspace. Construction is
/// cheap (creates directories on demand); the same handle can be shared
/// across threads via `Arc` — it holds no mutable state.
#[derive(Debug, Clone)]
pub struct WorkspaceCache {
    dir: PathBuf,
}

/// Size cap (bytes) for a single workspace's cache directory. At
/// startup, if the directory exceeds this, we reset it — simpler than
/// LRU eviction and the rebuild cost is bounded (it's just the next
/// workspace scan running as if cold). 512 MiB fits a mega-workspace
/// (50 k files × ~10 KB average `FileIndex`) with headroom and is
/// small enough that no reasonable disk will choke on it.
pub const CACHE_SIZE_CAP: u64 = 512 * 1024 * 1024;

/// A workspace cache unused for this long is deleted on the next startup.
/// Cap (bytes) on all workspace caches together. Past it, the least recently
/// used workspace directories are deleted until the total fits again.
pub const TOTAL_CACHE_SIZE_CAP: u64 = 2 * 1024 * 1024 * 1024;

const WORKSPACE_MAX_IDLE: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// Age after which a workspace directory without a meta file is deleted;
/// shields one that another instance has just created but not yet stamped.
const NO_META_GRACE: Duration = Duration::from_secs(24 * 60 * 60);

const META_FILE: &str = "workspace.json";

impl WorkspaceCache {
    /// Create (or re-open) the cache directory for a workspace rooted at
    /// `root`. Returns `None` when the system has no usable home/cache
    /// directory — callers should treat that as "cache disabled" and
    /// proceed without persistence.
    ///
    /// If the existing cache directory exceeds [`CACHE_SIZE_CAP`], it is
    /// cleared before the handle is returned. That's a coarse knob —
    /// K3 could refine to LRU-by-mtime — but crossing 512 MiB at
    /// startup indicates the workspace itself has more files than the
    /// cap fits, and the rebuild cost is bounded to one full
    /// re-scan.
    pub fn new(root: &Path) -> Option<Self> {
        let base = cache_base_dir()?;
        let schema = schema_version();
        let php_lsp_dir = base.join("php-lsp");
        let workspace = workspace_hash(root);
        let dir = php_lsp_dir.join(schema).join(&workspace);
        std::fs::create_dir_all(&dir).ok()?;
        let _ = write_workspace_meta(&dir, root);
        drop(prune_stale_dirs(php_lsp_dir, schema.to_string(), workspace));
        let cache = Self { dir };
        if cache.size_bytes().unwrap_or(0) > CACHE_SIZE_CAP {
            let _ = cache.clear();
        }
        Some(cache)
    }

    /// The filesystem path of this workspace's cache directory.
    pub fn cache_dir(&self) -> &std::path::Path {
        &self.dir
    }

    /// Total bytes consumed by this workspace's cache directory, including
    /// nested subdirectories — mir's `AnalysisCache` lives under a `session/`
    /// subdirectory of this one (see `set_session_cache_dir`), so the layout
    /// isn't flat even though this directory's own entries are.
    pub fn size_bytes(&self) -> io::Result<u64> {
        dir_size(&self.dir)
    }

    /// Override the root directory directly. The directory is used verbatim
    /// (no schema / workspace subdirectories are appended). Use for tests or
    /// when the caller provides an explicit `cachePath` initializationOption.
    pub fn with_dir(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// Build a cache key for `(uri, content)`. The filename half (`uri_hash`)
    /// stays fixed across edits to the same URI so a re-index overwrites its
    /// existing slot; the validation half (`content_hash`) changes on every
    /// edit so a read against stale content misses.
    pub fn key_for(uri: &str, content: &str) -> CacheKey {
        let uri_hash = blake3::hash(uri.as_bytes()).to_hex().as_str()[..32].to_string();
        let content_hash = blake3::hash(content.as_bytes()).to_hex().as_str()[..32].to_string();
        CacheKey {
            uri_hash,
            content_hash,
        }
    }

    /// Deserialize a previously-cached value. Returns `None` on any I/O
    /// or decode failure, or when the entry's stored content hash no
    /// longer matches `key` (the slot holds a stale revision) — all of
    /// which should look identical to a missing entry so callers fall
    /// through to the recompute path.
    pub fn read<T: DeserializeOwned>(&self, key: &CacheKey) -> Option<T> {
        let path = self.path_for(key);
        let bytes = std::fs::read(&path).ok()?;
        let config = bincode::config::standard();
        let (stored_hash, value): (String, T) =
            bincode::serde::decode_from_slice(&bytes, config).ok()?.0;
        if stored_hash != key.content_hash {
            return None;
        }
        Some(value)
    }

    /// Atomically publish an entry to the cache. Writes to a sibling
    /// temp file then renames, so readers never see a half-written
    /// payload even if the process dies mid-write. Because the filename
    /// is keyed on URI alone, this overwrites whatever revision (if any)
    /// previously occupied the slot rather than leaving it behind.
    ///
    /// No fsync: the cache is advisory-only — a crash that loses a write
    /// just produces a cache miss on the next startup, which safely falls
    /// back to re-parsing. Skipping sync_all() avoids 5–15 ms per file on
    /// macOS, which on a 1,500-file project accounts for most of the cold
    /// indexing time.
    pub fn write<T: Serialize>(&self, key: &CacheKey, value: &T) -> io::Result<()> {
        let path = self.path_for(key);
        let tmp = path.with_extension("tmp");
        let config = bincode::config::standard();
        let bytes = bincode::serde::encode_to_vec((&key.content_hash, value), config)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(&bytes)?;
        }
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// Drop every entry in this workspace's cache. Safe to call while
    /// other threads are reading — individual `read` calls that race
    /// with a `clear` will see `None` rather than garbage, and the next
    /// `write` recreates the entry.
    pub fn clear(&self) -> io::Result<()> {
        if self.dir.exists() {
            std::fs::remove_dir_all(&self.dir)?;
            std::fs::create_dir_all(&self.dir)?;
        }
        Ok(())
    }

    fn path_for(&self, key: &CacheKey) -> PathBuf {
        self.dir.join(format!("{}.bin", key.as_filename()))
    }
}

/// Recursively sums file sizes under `dir`. Used for the size cap: the
/// workspace cache directory isn't flat — mir's `AnalysisCache` lives in a
/// `session/` subdirectory underneath it — so a top-level-only scan would
/// never see it grow.
fn dir_size(dir: &Path) -> io::Result<u64> {
    let mut total = 0u64;
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e),
    };
    for entry in entries.flatten() {
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        if meta.is_dir() {
            total = total.saturating_add(dir_size(&entry.path())?);
        } else if meta.is_file() {
            total = total.saturating_add(meta.len());
        }
    }
    Ok(total)
}

/// Removes sibling schema-version directories under `php_lsp_dir` other than
/// `current_schema`, plus workspace directories under `current_schema` other
/// than `current_workspace` that [`is_stale_workspace_dir`] rejects, then
/// evicts least recently used workspaces past [`TOTAL_CACHE_SIZE_CAP`].
/// `schema_version()` embeds `CARGO_PKG_VERSION`, so every release moves to a
/// new schema directory, and every workspace ever opened (including temp
/// directories) gets its own; without this, nothing ever reads or removes
/// them. Only directories are touched — `php_lsp_dir` also holds flat sibling
/// files (e.g. `php-binary-version.json`, see
/// `autoload::detect_php_binary_version`) that must survive. Best-effort:
/// errors are ignored, same advisory-cache posture as the rest of this module.
///
/// Everything runs on a detached background thread: a stale schema directory
/// on a long-lived dev machine can hold millions of entries, and deleting it
/// inside `WorkspaceCache::new()` — on the same path as the workspace scan —
/// stalled startup for 20+ minutes before `indexReady` ever fired. Returns the
/// join handle so tests can wait for completion; normal callers drop it.
fn prune_stale_dirs(
    php_lsp_dir: PathBuf,
    current_schema: String,
    current_workspace: String,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let now = SystemTime::now();
        for dir in subdirs(&php_lsp_dir) {
            if dir.file_name() != Some(current_schema.as_ref()) {
                let _ = std::fs::remove_dir_all(dir);
            }
        }
        let schema_dir = php_lsp_dir.join(&current_schema);
        for dir in subdirs(&schema_dir) {
            if dir.file_name() != Some(current_workspace.as_ref())
                && is_stale_workspace_dir(&dir, now)
            {
                let _ = std::fs::remove_dir_all(dir);
            }
        }
        evict_lru_workspaces(&schema_dir, &current_workspace, TOTAL_CACHE_SIZE_CAP);
    })
}

/// Deletes workspace directories under `schema_dir`, least recently used
/// first, until their combined size is at most `cap`. `current_workspace`
/// counts toward the total but is never deleted.
fn evict_lru_workspaces(schema_dir: &Path, current_workspace: &str, cap: u64) {
    let mut total = 0u64;
    let mut candidates = Vec::new();
    for dir in subdirs(schema_dir) {
        let size = dir_size(&dir).unwrap_or(0);
        total = total.saturating_add(size);
        if dir.file_name() != Some(current_workspace.as_ref()) {
            let used = last_used(&dir, read_workspace_meta(&dir).as_ref());
            candidates.push((used, size, dir));
        }
    }
    candidates.sort_by_key(|(used, _, _)| *used);
    for (_, size, dir) in candidates {
        if total <= cap {
            break;
        }
        if std::fs::remove_dir_all(&dir).is_ok() {
            total -= size;
        }
    }
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
        .map(|entry| entry.path())
        .collect()
}

#[derive(Serialize, serde::Deserialize)]
struct WorkspaceMeta {
    root: PathBuf,
    last_used_secs: u64,
}

fn write_workspace_meta(dir: &Path, root: &Path) -> io::Result<()> {
    let meta = WorkspaceMeta {
        root: root.canonicalize().unwrap_or_else(|_| root.to_path_buf()),
        last_used_secs: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
    };
    let path = dir.join(META_FILE);
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(&meta).map_err(io::Error::other)?)?;
    std::fs::rename(&tmp, &path)
}

/// A workspace cache directory is stale when its recorded root is gone, or
/// when neither its meta stamp nor its own mtime (bumped by every entry
/// write, so a long-running instance keeps its directory fresh) falls within
/// [`WORKSPACE_MAX_IDLE`]. Without a readable meta file it only survives
/// [`NO_META_GRACE`].
fn is_stale_workspace_dir(dir: &Path, now: SystemTime) -> bool {
    let idle_since = |t: SystemTime| now.duration_since(t).unwrap_or(Duration::ZERO);
    let Some(meta) = read_workspace_meta(dir) else {
        return idle_since(last_used(dir, None)) > NO_META_GRACE;
    };
    if !meta.root.exists() {
        return true;
    }
    idle_since(last_used(dir, Some(&meta))) > WORKSPACE_MAX_IDLE
}

fn read_workspace_meta(dir: &Path) -> Option<WorkspaceMeta> {
    let bytes = std::fs::read(dir.join(META_FILE)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// The later of the meta stamp and the directory's mtime; an unreadable
/// mtime counts as "now" so an error never makes a directory look old.
fn last_used(dir: &Path, meta: Option<&WorkspaceMeta>) -> SystemTime {
    let mtime = std::fs::metadata(dir)
        .and_then(|m| m.modified())
        .unwrap_or_else(|_| SystemTime::now());
    let stamped = meta.map_or(UNIX_EPOCH, |m| {
        UNIX_EPOCH + Duration::from_secs(m.last_used_secs)
    });
    stamped.max(mtime)
}

/// Platform cache directory: `$XDG_CACHE_HOME` or `$HOME/.cache` on Unix,
/// `%LOCALAPPDATA%` on Windows. Deliberately doesn't depend on the `dirs`
/// crate — keeps the footprint small and the behaviour predictable.
pub(crate) fn cache_base_dir() -> Option<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_CACHE_HOME")
        && !xdg.is_empty()
    {
        return Some(PathBuf::from(xdg));
    }
    if cfg!(windows) {
        if let Some(local) = std::env::var_os("LOCALAPPDATA")
            && !local.is_empty()
        {
            return Some(PathBuf::from(local));
        }
    } else if let Some(home) = std::env::var_os("HOME")
        && !home.is_empty()
    {
        return Some(PathBuf::from(home).join(".cache"));
    }
    None
}

/// Schema marker: bumping `php-lsp`'s crate version invalidates every cached
/// entry (a new release moves to a new directory; see
/// `prune_stale_dirs` for cleanup of the old one). `fi-vN` is the one
/// manual knob — bump it whenever `FileIndex` or any type it contains gains,
/// loses, or renames a field, including a `php_ast`/parser upgrade that
/// changes what gets extracted. `FileIndex` has no dependency on any mir
/// type, so a mir version has no bearing on this cache's validity.
fn schema_version() -> &'static str {
    concat!(env!("CARGO_PKG_VERSION"), "-fi-v6")
}

fn workspace_hash(root: &Path) -> String {
    let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let hex = blake3::hash(canonical.as_os_str().as_encoded_bytes()).to_hex();
    hex.as_str()[..16].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[derive(Serialize, serde::Deserialize, PartialEq, Debug)]
    struct SamplePayload {
        name: String,
        values: Vec<u32>,
    }

    #[test]
    fn key_for_is_deterministic_per_uri_and_content() {
        let k1 = WorkspaceCache::key_for("file:///a.php", "<?php echo 1;");
        let k2 = WorkspaceCache::key_for("file:///a.php", "<?php echo 1;");
        assert_eq!(k1, k2);
    }

    #[test]
    fn key_for_differs_when_content_differs() {
        let k1 = WorkspaceCache::key_for("file:///a.php", "<?php echo 1;");
        let k2 = WorkspaceCache::key_for("file:///a.php", "<?php echo 2;");
        assert_ne!(k1, k2);
    }

    #[test]
    fn key_for_differs_when_uri_differs() {
        let k1 = WorkspaceCache::key_for("file:///a.php", "<?php");
        let k2 = WorkspaceCache::key_for("file:///b.php", "<?php");
        assert_ne!(k1, k2);
    }

    #[test]
    fn write_overwrites_slot_instead_of_orphaning_old_revision() {
        let dir = TempDir::new().unwrap();
        let cache = WorkspaceCache::with_dir(dir.path().to_path_buf());
        let uri = "file:///churn.php";

        let key_v1 = WorkspaceCache::key_for(uri, "<?php echo 1;");
        cache
            .write(
                &key_v1,
                &SamplePayload {
                    name: "v1".into(),
                    values: vec![1],
                },
            )
            .unwrap();

        let key_v2 = WorkspaceCache::key_for(uri, "<?php echo 2;");
        cache
            .write(
                &key_v2,
                &SamplePayload {
                    name: "v2".into(),
                    values: vec![2],
                },
            )
            .unwrap();

        // Same URI → same on-disk slot, so the second write replaces the
        // first rather than leaving a second, now-unreachable .bin file.
        let bin_files: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "bin"))
            .collect();
        assert_eq!(
            bin_files.len(),
            1,
            "editing a file must not orphan its previous cache entry"
        );

        // The stale key (old content) now misses instead of returning v1.
        let stale: Option<SamplePayload> = cache.read(&key_v1);
        assert!(stale.is_none());

        let current: SamplePayload = cache.read(&key_v2).unwrap();
        assert_eq!(current.name, "v2");
    }

    #[test]
    fn write_then_read_round_trips() {
        let dir = TempDir::new().unwrap();
        let cache = WorkspaceCache::with_dir(dir.path().to_path_buf());
        let key = WorkspaceCache::key_for("file:///x.php", "<?php");
        let payload = SamplePayload {
            name: "x".into(),
            values: vec![1, 2, 3],
        };
        cache.write(&key, &payload).unwrap();
        let decoded: SamplePayload = cache.read(&key).unwrap();
        assert_eq!(decoded, payload);
    }

    #[test]
    fn read_returns_none_for_missing_key() {
        let dir = TempDir::new().unwrap();
        let cache = WorkspaceCache::with_dir(dir.path().to_path_buf());
        let missing = WorkspaceCache::key_for("file:///nope.php", "");
        let decoded: Option<SamplePayload> = cache.read(&missing);
        assert!(decoded.is_none());
    }

    #[test]
    fn read_returns_none_for_corrupted_entry() {
        let dir = TempDir::new().unwrap();
        let cache = WorkspaceCache::with_dir(dir.path().to_path_buf());
        let key = WorkspaceCache::key_for("file:///c.php", "<?php");
        // Write garbage bytes directly into the slot the cache would use.
        std::fs::write(cache.path_for(&key), b"not valid bincode").unwrap();
        let decoded: Option<SamplePayload> = cache.read(&key);
        assert!(
            decoded.is_none(),
            "corrupted entry must look missing, not panic"
        );
    }

    #[test]
    fn write_is_atomic_via_rename() {
        // If the write path didn't go through a temp file, a crash
        // mid-`write_all` could leave a half-written `.bin`. We can't
        // easily simulate a crash, but we can at least assert the
        // temp-file doesn't linger on success.
        let dir = TempDir::new().unwrap();
        let cache = WorkspaceCache::with_dir(dir.path().to_path_buf());
        let key = WorkspaceCache::key_for("file:///atomic.php", "<?php");
        let payload = SamplePayload {
            name: "a".into(),
            values: vec![],
        };
        cache.write(&key, &payload).unwrap();
        let tmp = cache.path_for(&key).with_extension("tmp");
        assert!(!tmp.exists(), "tmp file should be removed by rename");
    }

    #[test]
    fn clear_drops_all_entries() {
        let dir = TempDir::new().unwrap();
        let cache = WorkspaceCache::with_dir(dir.path().to_path_buf());
        for i in 0..3 {
            let k = WorkspaceCache::key_for(&format!("file:///c{i}.php"), "");
            cache
                .write(
                    &k,
                    &SamplePayload {
                        name: i.to_string(),
                        values: vec![],
                    },
                )
                .unwrap();
        }
        cache.clear().unwrap();
        for i in 0..3 {
            let k = WorkspaceCache::key_for(&format!("file:///c{i}.php"), "");
            let decoded: Option<SamplePayload> = cache.read(&k);
            assert!(decoded.is_none());
        }
    }

    #[test]
    fn size_bytes_sums_flat_bin_files() {
        let dir = TempDir::new().unwrap();
        let cache = WorkspaceCache::with_dir(dir.path().to_path_buf());
        assert_eq!(cache.size_bytes().unwrap(), 0);

        let key1 = WorkspaceCache::key_for("file:///s1.php", "<?php");
        cache
            .write(
                &key1,
                &SamplePayload {
                    name: "s1".into(),
                    values: vec![0u32; 16],
                },
            )
            .unwrap();
        let key2 = WorkspaceCache::key_for("file:///s2.php", "<?php");
        cache
            .write(
                &key2,
                &SamplePayload {
                    name: "s2".into(),
                    values: vec![0u32; 16],
                },
            )
            .unwrap();

        let total = cache.size_bytes().unwrap();
        let expected1 = cache.path_for(&key1).metadata().unwrap().len();
        let expected2 = cache.path_for(&key2).metadata().unwrap().len();
        assert_eq!(total, expected1 + expected2);
    }

    #[test]
    fn size_bytes_recurses_into_nested_session_dir() {
        // Simulates mir's AnalysisCache living under `<workspace-hash>/session/`:
        // size_bytes must see it, not just this directory's own flat entries.
        let dir = TempDir::new().unwrap();
        let cache = WorkspaceCache::with_dir(dir.path().to_path_buf());

        let key = WorkspaceCache::key_for("file:///s.php", "<?php");
        cache
            .write(
                &key,
                &SamplePayload {
                    name: "s".into(),
                    values: vec![0u32; 16],
                },
            )
            .unwrap();

        let session_dir = dir.path().join("session");
        std::fs::create_dir_all(&session_dir).unwrap();
        std::fs::write(session_dir.join("cache.bin"), vec![0u8; 128]).unwrap();

        let flat_len = cache.path_for(&key).metadata().unwrap().len();
        assert_eq!(cache.size_bytes().unwrap(), flat_len + 128);
    }

    #[test]
    fn file_index_round_trips() {
        use crate::document::ast::ParsedDoc;
        use crate::index::file_index::FileIndex;

        let dir = TempDir::new().unwrap();
        let cache = WorkspaceCache::with_dir(dir.path().to_path_buf());
        let src = "<?php\nnamespace App;\nclass Foo { public function bar(): string {} }";
        let key = WorkspaceCache::key_for("file:///Foo.php", src);

        let doc = ParsedDoc::parse(src.to_string());
        let index = FileIndex::extract(&doc);
        cache.write(&key, &index).unwrap();

        let decoded: FileIndex = cache.read(&key).unwrap();
        assert_eq!(decoded.namespace.as_deref(), Some("App"));
        assert_eq!(decoded.classes.len(), 1);
        assert_eq!(decoded.classes[0].name.as_ref(), "Foo");
        assert_eq!(decoded.classes[0].methods.len(), 1);
        assert_eq!(decoded.classes[0].methods[0].name.as_ref(), "bar");
    }

    #[test]
    fn prune_stale_dirs_removes_old_schemas_keeps_current_and_flat_files() {
        let dir = TempDir::new().unwrap();
        let php_lsp_dir = dir.path();

        let stale = php_lsp_dir.join("0.21.0-fi-v3");
        std::fs::create_dir_all(stale.join("workspacehash")).unwrap();
        std::fs::write(stale.join("workspacehash").join("a.bin"), b"old").unwrap();

        let current = php_lsp_dir.join("0.22.0-fi-v4");
        std::fs::create_dir_all(&current).unwrap();

        // Sibling flat file, like `php-binary-version.json` — must survive.
        std::fs::write(php_lsp_dir.join("php-binary-version.json"), b"{}").unwrap();

        prune_stale_dirs(
            php_lsp_dir.to_path_buf(),
            "0.22.0-fi-v4".into(),
            "current".into(),
        )
        .join()
        .unwrap();

        assert!(!stale.exists(), "stale schema directory must be removed");
        assert!(current.exists(), "current schema directory must survive");
        assert!(
            php_lsp_dir.join("php-binary-version.json").exists(),
            "flat sibling files must not be touched"
        );
    }

    fn workspace_dir_with_meta(parent: &Path, root: &Path) -> PathBuf {
        let dir = parent.join(workspace_hash(root));
        std::fs::create_dir_all(&dir).unwrap();
        write_workspace_meta(&dir, root).unwrap();
        dir
    }

    #[test]
    fn workspace_dir_is_stale_once_its_root_is_deleted() {
        let cache = TempDir::new().unwrap();
        let root = TempDir::new().unwrap();
        let dir = workspace_dir_with_meta(cache.path(), root.path());
        assert!(!is_stale_workspace_dir(&dir, SystemTime::now()));

        drop(root);
        assert!(is_stale_workspace_dir(&dir, SystemTime::now()));
    }

    #[test]
    fn workspace_dir_is_stale_after_max_idle() {
        let cache = TempDir::new().unwrap();
        let root = TempDir::new().unwrap();
        let dir = workspace_dir_with_meta(cache.path(), root.path());
        let now = SystemTime::now();
        assert!(!is_stale_workspace_dir(&dir, now + WORKSPACE_MAX_IDLE / 2));
        assert!(is_stale_workspace_dir(
            &dir,
            now + WORKSPACE_MAX_IDLE + Duration::from_secs(60)
        ));
    }

    #[test]
    fn workspace_dir_without_meta_survives_only_the_grace_period() {
        let cache = TempDir::new().unwrap();
        let dir = cache.path().join("legacy");
        std::fs::create_dir_all(&dir).unwrap();
        let now = SystemTime::now();
        assert!(!is_stale_workspace_dir(&dir, now));
        assert!(is_stale_workspace_dir(
            &dir,
            now + NO_META_GRACE + Duration::from_secs(60)
        ));
    }

    #[test]
    fn prune_stale_dirs_removes_orphaned_workspaces_keeps_current_and_live() {
        let php_lsp_dir = TempDir::new().unwrap();
        let schema_dir = php_lsp_dir.path().join("0.22.0-fi-v4");

        let live_root = TempDir::new().unwrap();
        let live = workspace_dir_with_meta(&schema_dir, live_root.path());

        let gone_root = TempDir::new().unwrap();
        let orphaned = workspace_dir_with_meta(&schema_dir, gone_root.path());
        drop(gone_root);

        // The current workspace survives even with a deleted root.
        let current_root = TempDir::new().unwrap();
        let current = workspace_dir_with_meta(&schema_dir, current_root.path());
        drop(current_root);

        prune_stale_dirs(
            php_lsp_dir.path().to_path_buf(),
            "0.22.0-fi-v4".into(),
            current.file_name().unwrap().to_string_lossy().into_owned(),
        )
        .join()
        .unwrap();

        assert!(live.exists());
        assert!(current.exists());
        assert!(!orphaned.exists());
    }

    #[test]
    fn evict_lru_workspaces_drops_oldest_until_under_cap() {
        let schema_dir = TempDir::new().unwrap();
        let root = TempDir::new().unwrap();
        // Directory mtimes are all "now", so only a future stamp sorts newer.
        let stamps = [u64::from(u32::MAX), 0, 0];
        let dirs: Vec<PathBuf> = stamps
            .iter()
            .enumerate()
            .map(|(i, &last_used_secs)| {
                let dir = schema_dir.path().join(format!("ws{i}"));
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(dir.join("a.bin"), vec![0u8; 1000]).unwrap();
                let meta = WorkspaceMeta {
                    root: root.path().to_path_buf(),
                    last_used_secs,
                };
                std::fs::write(dir.join(META_FILE), serde_json::to_vec(&meta).unwrap()).unwrap();
                dir
            })
            .collect();

        // ws2 is current: counted, never deleted, so the older ws1 goes.
        let per_dir = dir_size(&dirs[0]).unwrap();
        evict_lru_workspaces(schema_dir.path(), "ws2", 2 * per_dir + per_dir / 2);

        assert!(dirs[0].exists());
        assert!(!dirs[1].exists());
        assert!(dirs[2].exists());
    }
}
