//! Persistent on-disk cache for Phase K.
//!
//! **Status: infrastructure layer only.** This module exposes the primitives
//! — directory layout, content hashing, serde round-trip — that a later
//! commit will wire into `scan_workspace` to skip re-parsing on warm start.
//! Nothing in `backend.rs` / `document_store.rs` consumes it yet.
//!
//! ## Layout
//!
//! ```text
//! ~/.cache/php-lsp/<schema-version>/<workspace-hash>/<entry-hash>.bin
//! ```
//!
//! - `<schema-version>` — `php-lsp` crate version concatenated with the
//!   `mir-codebase` version (the latter owns `StubSlice`'s schema, so
//!   bumping either rotates the cache).
//! - `<workspace-hash>` — blake3 of the canonicalized absolute path of the
//!   first workspace root, truncated to 16 hex chars. Two separate projects
//!   get isolated caches; two checkouts of the same project at the same
//!   absolute path share one.
//! - `<entry-hash>` — blake3 of the bytes `uri || 0x00 || content`, truncated
//!   to 32 hex chars. Editing a file changes the content → new key → cache
//!   miss; a different file at the same URI also gets a different key.
//!
//! ## Format
//!
//! `bincode` v2 (binary, fast, schema-stable via serde derives on
//! `StubSlice` et al). Files are written atomically via a temp-file rename
//! to avoid half-written entries on an interrupted shutdown.
//!
//! ## Invalidation
//!
//! Rotating the schema version invalidates everything; rotating the content
//! invalidates one file. There's no LRU or cleanup yet — Step 2 will add a
//! size cap + orphan sweep.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Serialize, de::DeserializeOwned};

/// Identifies a single cache entry. Opaque — callers produce it via
/// [`WorkspaceCache::key_for`] and pass it straight back to read/write.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CacheKey(String);

impl CacheKey {
    fn as_filename(&self) -> &str {
        &self.0
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
/// (50 k files × ~10 KB average `StubSlice`) with headroom and is
/// small enough that no reasonable disk will choke on it.
pub const CACHE_SIZE_CAP: u64 = 512 * 1024 * 1024;

impl WorkspaceCache {
    /// Create (or re-open) the cache directory for a workspace rooted at
    /// `root`. Returns `None` when the system has no usable home/cache
    /// directory — callers should treat that as "cache disabled" and
    /// proceed without persistence.
    ///
    /// If the existing cache directory exceeds [`CACHE_SIZE_CAP`], it is
    /// cleared before the handle is returned. That's a coarse knob —
    /// K3 could refine to LRU-by-mtime — but crossing 512 MiB at
    /// startup indicates the workspace has churned through many
    /// content hashes and the rebuild cost is bounded to one full
    /// re-scan.
    pub fn new(root: &Path) -> Option<Self> {
        let base = cache_base_dir()?;
        let schema = schema_version();
        let workspace = workspace_hash(root);
        let dir = base.join("php-lsp").join(schema).join(workspace);
        std::fs::create_dir_all(&dir).ok()?;
        let cache = Self { dir };
        if cache.size_bytes().unwrap_or(0) > CACHE_SIZE_CAP {
            let _ = cache.clear();
        }
        Some(cache)
    }

    /// Total bytes consumed by `.bin` entries in this workspace's cache
    /// directory. Cheap (one `read_dir` pass, no recursion into
    /// subdirectories because the layout is flat).
    pub fn size_bytes(&self) -> io::Result<u64> {
        let mut total = 0u64;
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(e),
        };
        for entry in entries.flatten() {
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            if meta.is_file() {
                total = total.saturating_add(meta.len());
            }
        }
        Ok(total)
    }

    /// Override the root directory directly. Intended for tests; the
    /// directory is used verbatim (no schema / workspace subdirectories
    /// are appended).
    #[cfg(test)]
    pub fn with_dir(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// Build a cache key for a single file. Combines `uri` and `content`
    /// so that two files with identical content but different URIs get
    /// different keys (StubSlice bakes `file` into its payload).
    pub fn key_for(uri: &str, content: &str) -> CacheKey {
        let mut hasher = blake3::Hasher::new();
        hasher.update(uri.as_bytes());
        hasher.update(&[0u8]);
        hasher.update(content.as_bytes());
        let full = hasher.finalize().to_hex();
        // 32 hex chars = 128 bits, ample collision resistance for
        // workspaces with millions of files (birthday bound ≫ 10^18).
        CacheKey(full.as_str()[..32].to_string())
    }

    /// Deserialize a previously-cached value. Returns `None` on any I/O
    /// or decode failure — a corrupted entry should look identical to a
    /// missing one so callers fall through to the recompute path.
    pub fn read<T: DeserializeOwned>(&self, key: &CacheKey) -> Option<T> {
        let path = self.path_for(key);
        let bytes = std::fs::read(&path).ok()?;
        let config = bincode::config::standard();
        bincode::serde::decode_from_slice(&bytes, config)
            .ok()
            .map(|(v, _len)| v)
    }

    /// Atomically publish an entry to the cache. Writes to a sibling
    /// temp file then renames, so readers never see a half-written
    /// payload even if the process dies mid-write.
    pub fn write<T: Serialize>(&self, key: &CacheKey, value: &T) -> io::Result<()> {
        let path = self.path_for(key);
        let tmp = path.with_extension("tmp");
        let config = bincode::config::standard();
        let bytes = bincode::serde::encode_to_vec(value, config)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(&bytes)?;
            f.sync_all()?;
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

/// Platform cache directory: `$XDG_CACHE_HOME` or `$HOME/.cache` on Unix,
/// `%LOCALAPPDATA%` on Windows. Deliberately doesn't depend on the `dirs`
/// crate — keeps the footprint small and the behaviour predictable.
fn cache_base_dir() -> Option<PathBuf> {
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

/// Schema marker: bumping either `php-lsp` or `mir-codebase` invalidates
/// every cached entry. The hardcoded mir version is a trade-off: keeping
/// it in source means we don't depend on `build.rs` introspection, at the
/// cost of needing to remember to update it alongside `Cargo.toml`. A
/// compile-time assert in the serialize/deserialize path could catch
/// drift — deferred to Step 2.
fn schema_version() -> &'static str {
    concat!(env!("CARGO_PKG_VERSION"), "-mir-0.7")
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
        // Same content, different URI — the separator byte prevents
        // (uri_a || content_b) from colliding with (uri_a+b || content).
        let k1 = WorkspaceCache::key_for("file:///a.php", "<?php");
        let k2 = WorkspaceCache::key_for("file:///b.php", "<?php");
        assert_ne!(k1, k2);
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
    fn stub_slice_round_trips() {
        // Smoke-test the real payload shape Phase K Step 2 will cache:
        // mir_codebase::StubSlice already derives Serialize/Deserialize.
        let dir = TempDir::new().unwrap();
        let cache = WorkspaceCache::with_dir(dir.path().to_path_buf());
        let key = WorkspaceCache::key_for("file:///stub.php", "<?php class Foo {}");
        let slice = mir_codebase::storage::StubSlice::default();
        cache.write(&key, &slice).unwrap();
        let decoded: mir_codebase::storage::StubSlice = cache.read(&key).unwrap();
        // StubSlice has no PartialEq, so we compare a cheap proxy:
        // the class count (0 for a default).
        assert_eq!(decoded.classes.len(), slice.classes.len());
    }
}
