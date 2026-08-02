use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use dashmap::DashMap;
use tower_lsp_server::ls_types::{SemanticToken, Uri};

use crate::document::ast::ParsedDoc;
use crate::index::file_index::FileIndex;

/// Upper bound on `parsed_cache` entries. Matched to the `lru = 2048` on
/// `parsed_doc` in `src/db/parse.rs` so the secondary Arc retention can't
/// pin more ASTs alive than salsa's memo already bounds.
pub(crate) const PARSED_CACHE_CAP: usize = 2048;

/// Upper bounds for the heavy per-file caches that otherwise grow with every
/// distinct file touched across a session (`FileAnalysis` ~50 KiB, owned
/// `Program` ~100 KiB). Entries self-evict on edit but never on file count,
/// so multi-hour sessions accumulate them unboundedly without these caps.
pub(crate) const ANALYSIS_CACHE_CAP: usize = 512;
pub(crate) const OWNED_PROGRAM_CACHE_CAP: usize = 256;

/// All per-file caches owned by `DocumentStore`, grouped so eviction logic
/// lives in one place. Adding a new cache only requires: add the field here,
/// then add `self.field.remove(uri)` to `evict()`.
pub(crate) struct CacheRegistry {
    /// Cached semantic tokens per document: (result_id, tokens).
    pub(crate) token_cache: DashMap<Uri, (String, Arc<Vec<SemanticToken>>)>,
    /// G2: lock-free mirror of each file's last-set text for dedup in mirror_text.
    pub(crate) text_cache: DashMap<Uri, Arc<str>>,
    /// G3: cross-revision read-through cache for parsed_doc.
    pub(crate) parsed_cache: DashMap<Uri, (Arc<str>, Arc<ParsedDoc>)>,
    /// Per-file mir body analysis cache: (source_arc, decl_ver, analysis).
    pub(crate) analysis_cache: DashMap<Uri, (Arc<str>, u64, Arc<mir_analyzer::FileAnalysis>)>,
    /// Monotonically increasing counter bumped on any declaration-level change.
    pub(crate) decl_version: AtomicU64,
    /// Count of real `ParsedDoc` parses served by `get_parsed_cached` (cache
    /// misses only). Read via `$/php-lsp/debugStats` to guard the references
    /// read path against re-introducing whole-workspace parsing.
    pub(crate) parse_count: AtomicU64,
    /// Last-seen FileIndex per URI, used to detect declaration changes.
    pub(crate) decl_fingerprints: DashMap<Uri, Arc<FileIndex>>,
    /// Owned-program cache: (source_arc, owned_program). Avoids repeating the
    /// deep arena clone in `cached_analysis` when `decl_version` bumps due to
    /// a sibling file's declaration change — the file's own source is unchanged,
    /// so the owned AST copy can be reused.
    pub(crate) owned_program_cache: DashMap<Uri, (Arc<str>, Arc<php_ast::owned::Program>)>,
    /// On-demand `FileIndex` store for vendor files loaded lazily via PSR-4
    /// navigation. Vendor is excluded from the eager workspace scan; files
    /// ingested by `psr4_method_goto` are not in the salsa workspace_index.
    /// Evicted alongside all other per-file caches via `evict()`.
    pub(crate) vendor_index_cache: DashMap<Uri, Arc<FileIndex>>,
    /// Monotonic counter driving `last_access`.
    access_tick: AtomicU64,
    /// Last-use tick per file, shared by every bounded per-file cache. Updated
    /// on cache hits and inserts; `shed_stale` drops the least-recently-used
    /// half of a cache that has reached its cap. Shared recency is deliberate:
    /// all per-file caches correlate with "this file was recently involved in
    /// a request", and one map keeps the bookkeeping off the hot paths.
    last_access: DashMap<Uri, u64>,
}

impl CacheRegistry {
    pub(crate) fn new() -> Self {
        CacheRegistry {
            token_cache: DashMap::new(),
            text_cache: DashMap::new(),
            parsed_cache: DashMap::new(),
            analysis_cache: DashMap::new(),
            decl_version: AtomicU64::new(0),
            parse_count: AtomicU64::new(0),
            decl_fingerprints: DashMap::new(),
            owned_program_cache: DashMap::new(),
            vendor_index_cache: DashMap::new(),
            access_tick: AtomicU64::new(0),
            last_access: DashMap::new(),
        }
    }

    /// Record a use of `uri`'s per-file caches (hit or insert). Recency feeds
    /// [`Self::shed_stale`]. Called 2-3x per URI on every cache-hit request
    /// path (hover, completion, code_lens, ...), so the common case must not
    /// pay a `Uri` clone: `get_mut` looks up by `&Uri` and only allocates on
    /// the (rare) first-ever touch of a URI.
    pub(crate) fn touch(&self, uri: &Uri) {
        let tick = self.access_tick.fetch_add(1, Ordering::Relaxed);
        if let Some(mut existing) = self.last_access.get_mut(uri) {
            *existing = tick;
        } else {
            self.last_access.insert(uri.clone(), tick);
        }
    }

    /// When `map` has reached `cap`, drop the least-recently-touched half.
    /// Entries with no recorded access sort oldest and shed first.
    ///
    /// Two passes instead of a single clone-everything-then-sort: pass 1
    /// collects only ticks (`u64`, `Copy` — no `Uri` clones) and partitions
    /// them in O(n) via `select_nth_unstable` instead of an O(n log n) full
    /// sort; pass 2 clones only the `Uri`s actually being evicted (~`cap/2`)
    /// instead of every entry in the map. Both passes fully drain
    /// `map.iter()` into an owned `Vec` before any `map.remove()` call —
    /// removing while a `DashMap` iterator holds that shard's read guard
    /// would deadlock.
    pub(crate) fn shed_stale<V>(&self, map: &DashMap<Uri, V>, cap: usize) {
        let len = map.len();
        if len < cap {
            return;
        }
        let to_evict = cap / 2;
        if to_evict == 0 {
            return;
        }

        let mut ticks: Vec<u64> = map
            .iter()
            .map(|e| self.last_access.get(e.key()).map(|t| *t).unwrap_or(0))
            .collect();
        let (_, &mut threshold, _) = ticks.select_nth_unstable(to_evict - 1);

        // Tied entries at exactly `threshold` may exceed `to_evict` in count;
        // `take(to_evict)` bounds the eviction to the target regardless —
        // acceptable for this soft/approximate LRU policy (the old full sort
        // over an unstable comparator wasn't a strict tie-break contract
        // either).
        let to_remove: Vec<Uri> = map
            .iter()
            .filter(|e| {
                let tick = self.last_access.get(e.key()).map(|t| *t).unwrap_or(0);
                tick <= threshold
            })
            .take(to_evict)
            .map(|e| e.key().clone())
            .collect();
        for uri in &to_remove {
            map.remove(uri);
        }
    }

    /// Evict every per-file cache entry for `uri`. Call this from `DocumentStore::remove`.
    pub(crate) fn evict(&self, uri: &Uri) {
        self.token_cache.remove(uri);
        self.text_cache.remove(uri);
        self.parsed_cache.remove(uri);
        self.analysis_cache.remove(uri);
        self.decl_fingerprints.remove(uri);
        self.owned_program_cache.remove(uri);
        self.vendor_index_cache.remove(uri);
        self.last_access.remove(uri);
    }

    /// Evict only the mir analysis cache for `uri`. Used on text change so the
    /// next request re-runs Pass 1 + Pass 2 with the new content.
    pub(crate) fn evict_analysis(&self, uri: &Uri) {
        self.analysis_cache.remove(uri);
    }

    /// Clear the entire analysis cache. Used when the PHP version or
    /// autoload.files set changes, making all cached FileAnalysis stale.
    pub(crate) fn evict_analysis_all(&self) {
        self.analysis_cache.clear();
    }

    /// Evict only the semantic-tokens cache for `uri`. Used when a file is
    /// closed; delta tokens computed against the old revision are invalid.
    pub(crate) fn evict_tokens(&self, uri: &Uri) {
        self.token_cache.remove(uri);
    }

    /// Store a fresh token set for delta requests.
    pub(crate) fn store_token(
        &self,
        uri: &Uri,
        result_id: String,
        tokens: Arc<Vec<SemanticToken>>,
    ) {
        self.token_cache.insert(uri.clone(), (result_id, tokens));
    }

    /// Return the cached token set if `result_id` matches.
    pub(crate) fn get_token(&self, uri: &Uri, result_id: &str) -> Option<Arc<Vec<SemanticToken>>> {
        self.token_cache
            .get(uri)
            .filter(|e| e.0.as_str() == result_id)
            .map(|e| Arc::clone(&e.1))
    }

    /// Publish a fresh `ParsedDoc` into `parsed_cache`, shedding the
    /// least-recently-used half first when it has grown past
    /// [`PARSED_CACHE_CAP`]. Recency-based (not arbitrary): a references
    /// sweep over a large candidate set must not evict the open files the
    /// user is actively editing.
    pub(crate) fn insert_parsed(&self, uri: Uri, text: Arc<str>, doc: Arc<ParsedDoc>) {
        self.shed_stale(&self.parsed_cache, PARSED_CACHE_CAP);
        self.touch(&uri);
        self.parsed_cache.insert(uri, (text, doc));
    }

    pub(crate) fn decl_version(&self) -> u64 {
        self.decl_version.load(Ordering::Acquire)
    }

    pub(crate) fn bump_decl_version(&self) {
        self.decl_version.fetch_add(1, Ordering::Release);
    }

    pub(crate) fn bump_parse_count(&self) {
        self.parse_count.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn parse_count(&self) -> u64 {
        self.parse_count.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri(n: usize) -> Uri {
        format!("file:///f{n}.php").parse::<Uri>().unwrap()
    }

    #[test]
    fn shed_stale_noop_below_cap() {
        let reg = CacheRegistry::new();
        let map: DashMap<Uri, ()> = DashMap::new();
        for i in 0..5 {
            map.insert(uri(i), ());
            reg.touch(&uri(i));
        }
        reg.shed_stale(&map, 10);
        assert_eq!(map.len(), 5, "below cap: nothing evicted");
        for i in 0..5 {
            assert!(
                map.contains_key(&uri(i)),
                "uri({i}) should still be present"
            );
        }
    }

    #[test]
    fn shed_stale_evicts_oldest_half_at_cap() {
        let reg = CacheRegistry::new();
        let map: DashMap<Uri, ()> = DashMap::new();
        // Insert in order so uri(0) is least-recently-touched, uri(9) most recent.
        for i in 0..10 {
            map.insert(uri(i), ());
            reg.touch(&uri(i));
        }
        reg.shed_stale(&map, 10);
        assert_eq!(map.len(), 5, "half of cap should be evicted");
        for i in 0..5 {
            assert!(
                !map.contains_key(&uri(i)),
                "uri({i}) is oldest, should be evicted"
            );
        }
        for i in 5..10 {
            assert!(
                map.contains_key(&uri(i)),
                "uri({i}) is newest, should survive"
            );
        }
    }

    #[test]
    fn shed_stale_treats_untouched_entries_as_oldest() {
        let reg = CacheRegistry::new();
        let map: DashMap<Uri, ()> = DashMap::new();
        // A CacheRegistry's very first touch() returns tick 0 — the same
        // value `unwrap_or(0)` uses for a never-touched entry. Burn that
        // first tick on a throwaway key so uri(2)/uri(3) below get ticks
        // that unambiguously outrank the untouched default.
        reg.touch(&uri(999));
        // uri(0)/uri(1) inserted but never touched (tick defaults to 0);
        // uri(2)/uri(3) touched, so they're more recent.
        map.insert(uri(0), ());
        map.insert(uri(1), ());
        map.insert(uri(2), ());
        reg.touch(&uri(2));
        map.insert(uri(3), ());
        reg.touch(&uri(3));
        reg.shed_stale(&map, 4);
        assert_eq!(map.len(), 2);
        assert!(!map.contains_key(&uri(0)));
        assert!(!map.contains_key(&uri(1)));
        assert!(map.contains_key(&uri(2)));
        assert!(map.contains_key(&uri(3)));
    }
}
