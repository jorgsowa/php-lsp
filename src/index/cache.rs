//! Per-workspace on-disk cache directory.
//!
//! php-lsp stores no per-file entries of its own: the directory hosts mir's
//! caches (analysis results and per-file definitions) under `session/`, and
//! this module owns its location, rotation and cleanup.
//!
//! ## Layout
//!
//! ```text
//! ~/.cache/php-lsp/<schema-version>/<workspace-hash>/session/
//! ```
//!
//! - `<schema-version>` — `php-lsp` crate version; bumping it rotates the
//!   entire cache. Another version's directory is pruned by
//!   `WorkspaceCache::new` once none of its workspaces has been used for a
//!   day, so two installed versions running side by side keep their own caches.
//! - `<workspace-hash>` — blake3 of the canonicalized absolute path of the
//!   first workspace root, truncated to 16 hex chars. Two separate projects
//!   get isolated caches; two checkouts of the same project at the same
//!   absolute path share one. Holds a `workspace.json` recording the root
//!   path and last-used time, so the background pruner can drop caches for
//!   roots that no longer exist or haven't been opened in
//!   [`WORKSPACE_MAX_IDLE`].
//!
//! The size cap in [`WorkspaceCache::new`] is a backstop for workspaces that
//! outgrow it, not the primary cleanup mechanism.

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
/// workspace scan running as if cold). 512 MiB fits mir's caches for a
/// mega-workspace with headroom and is small enough that no reasonable disk
/// will choke on it.
pub const CACHE_SIZE_CAP: u64 = 512 * 1024 * 1024;

/// A workspace cache unused for this long is deleted on the next startup.
/// Cap (bytes) on all workspace caches together. Past it, the least recently
/// used workspace directories are deleted until the total fits again.
pub const TOTAL_CACHE_SIZE_CAP: u64 = 2 * 1024 * 1024 * 1024;

const WORKSPACE_MAX_IDLE: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// Age after which a workspace directory without a meta file is deleted;
/// shields one that another instance has just created but not yet stamped.
const NO_META_GRACE: Duration = Duration::from_secs(24 * 60 * 60);

/// How long another version's schema directory survives after its last use.
const OTHER_SCHEMA_GRACE: Duration = Duration::from_secs(24 * 60 * 60);

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
        let dir = Self::open_in(&base, root)?.dir;
        let _ = write_workspace_meta(&dir, root);
        drop(prune_stale_dirs(php_lsp_dir, schema.to_string(), workspace));
        let cache = Self { dir };
        if cache.size_bytes().unwrap_or(0) > CACHE_SIZE_CAP {
            let _ = cache.clear();
        }
        Some(cache)
    }

    fn open_in(base: &Path, root: &Path) -> Option<Self> {
        let dir = base
            .join("php-lsp")
            .join(schema_version())
            .join(workspace_hash(root));
        std::fs::create_dir_all(&dir).ok()?;
        Some(Self { dir })
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
/// `current_schema` that no instance has used within [`OTHER_SCHEMA_GRACE`], plus workspace directories under `current_schema` other
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
        prune_stale_dirs_at(
            &php_lsp_dir,
            &current_schema,
            &current_workspace,
            SystemTime::now(),
        );
    })
}

fn prune_stale_dirs_at(
    php_lsp_dir: &Path,
    current_schema: &str,
    current_workspace: &str,
    now: SystemTime,
) {
    for dir in subdirs(php_lsp_dir) {
        if dir.file_name() != Some(current_schema.as_ref()) && !schema_dir_in_use(&dir, now) {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
    let schema_dir = php_lsp_dir.join(current_schema);
    for dir in subdirs(&schema_dir) {
        if dir.file_name() != Some(current_workspace.as_ref()) && is_stale_workspace_dir(&dir, now)
        {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
    evict_lru_workspaces(&schema_dir, current_workspace, TOTAL_CACHE_SIZE_CAP);
}

/// Another php-lsp version may be running against its own schema directory;
/// keep it while any of its workspaces was used within [`OTHER_SCHEMA_GRACE`].
fn schema_dir_in_use(schema_dir: &Path, now: SystemTime) -> bool {
    subdirs(schema_dir).iter().any(|dir| {
        let used = last_used(dir, read_workspace_meta(dir).as_ref());
        now.duration_since(used).unwrap_or(Duration::ZERO) <= OTHER_SCHEMA_GRACE
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

#[derive(serde::Serialize, serde::Deserialize)]
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
/// entry (a new release moves to a new directory; see `prune_stale_dirs` for
/// cleanup of the old one). `vN` rotates the layout between releases.
fn schema_version() -> &'static str {
    concat!(env!("CARGO_PKG_VERSION"), "-v7")
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

    #[test]
    fn clear_drops_all_entries() {
        let dir = TempDir::new().unwrap();
        let cache = WorkspaceCache::with_dir(dir.path().to_path_buf());
        std::fs::write(dir.path().join("a.bin"), b"x").unwrap();
        std::fs::create_dir_all(dir.path().join("session")).unwrap();
        cache.clear().unwrap();
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn size_bytes_recurses_into_nested_session_dir() {
        // mir's caches live under `<workspace-hash>/session/`.
        let dir = TempDir::new().unwrap();
        let cache = WorkspaceCache::with_dir(dir.path().to_path_buf());
        assert_eq!(cache.size_bytes().unwrap(), 0);
        std::fs::write(dir.path().join(META_FILE), vec![0u8; 16]).unwrap();
        let session_dir = dir.path().join("session");
        std::fs::create_dir_all(&session_dir).unwrap();
        std::fs::write(session_dir.join("cache.bin"), vec![0u8; 128]).unwrap();
        assert_eq!(cache.size_bytes().unwrap(), 16 + 128);
    }

    #[test]
    fn open_in_leaves_other_cache_dirs_alone() {
        let base = TempDir::new().unwrap();
        let root = TempDir::new().unwrap();
        let other_schema = base.path().join("php-lsp").join("0.0.0-old");
        std::fs::create_dir_all(&other_schema).unwrap();

        let cache = WorkspaceCache::open_in(base.path(), root.path()).unwrap();

        assert!(cache.cache_dir().is_dir());
        assert!(!cache.cache_dir().join(META_FILE).exists());
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(other_schema.is_dir());
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

        prune_stale_dirs_at(
            php_lsp_dir,
            "0.22.0-fi-v4",
            "current",
            SystemTime::now() + OTHER_SCHEMA_GRACE + Duration::from_secs(60),
        );

        assert!(!stale.exists(), "stale schema directory must be removed");
        assert!(current.exists(), "current schema directory must survive");
        assert!(
            php_lsp_dir.join("php-binary-version.json").exists(),
            "flat sibling files must not be touched"
        );
    }

    #[test]
    fn prune_stale_dirs_keeps_schema_dir_another_version_is_using() {
        let php_lsp_dir = TempDir::new().unwrap();
        let root = TempDir::new().unwrap();
        let other = php_lsp_dir.path().join("0.21.0-fi-v3");
        workspace_dir_with_meta(&other, root.path());
        let empty = php_lsp_dir.path().join("0.20.0-fi-v3");
        std::fs::create_dir_all(&empty).unwrap();

        prune_stale_dirs(
            php_lsp_dir.path().to_path_buf(),
            "0.22.0-fi-v4".into(),
            "current".into(),
        )
        .join()
        .unwrap();

        assert!(
            other.exists(),
            "a recently used schema directory must survive"
        );
        assert!(
            !empty.exists(),
            "a schema directory with no workspaces is unused"
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
