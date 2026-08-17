use std::sync::Arc;

use tower_lsp_server::jsonrpc::Result;
use tower_lsp_server::ls_types::*;

use crate::analysis::document_highlight::document_highlights;
use crate::lang::is_unresolvable_bareword_at;
use crate::navigation::definition::{
    find_declaration_range, find_method_in_class_hierarchy, find_method_range_in_class,
    find_property_in_class_hierarchy,
};
use crate::navigation::references::{
    build_mir_symbol, dedup_ref_locations, session_tuple_to_location,
};
use crate::navigation::walk::collect_var_refs_in_scope;
use crate::text::{fqn_short_name, utf16_code_units, utf16_offset_to_byte, word_at_position};
use crate::types::type_map::{enclosing_class_at, enclosing_class_fqn_at};

use super::super::helpers::{
    class_name_at_construct_decl, promoted_property_at_cursor, range_within,
};
use super::super::panic_guard::guard_async_result;
use super::super::{Backend, class_before_double_colon, resolve_reference_symbol};

impl Backend {
    pub(crate) async fn handle_goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        guard_async_result("goto_definition", async move {
            let uri = &params.text_document_position_params.text_document.uri;
            let position = params.text_document_position_params.position;
            let source = self.get_open_text(uri).unwrap_or_default();

            let doc = match self.get_doc(uri) {
                Some(d) => d,
                None => return Ok(None),
            };
            // Reused across the fallback branches below when no lazy vendor
            // ingestion happens in between (the common case) — collapses to
            // one workspace-index fetch instead of up to three. Reset to
            // `None` after any `psr4_method_goto`/`psr4_goto` call that can
            // lazily ingest a new file, so a later branch never reads a
            // pre-ingestion snapshot.
            let mut wi_cache: Option<Arc<crate::db::workspace_index::WorkspaceIndexData>> = None;

            // Laravel string-key calls (`env('KEY')`, `config('a.b')`, ...) —
            // resolved before falling through to symbol resolution, since a
            // string literal is never a `word_at_position` match.
            // `resolve_string_key` checks `is_laravel` first (one atomic load
            // + bool check) so non-Laravel workspaces never pay for the AST
            // walk inside it.
            let laravel = self.laravel.load();
            let laravel_loc =
                crate::laravel::resolve_string_key(&doc, position, &laravel).or_else(|| {
                    crate::laravel::blade::resolve_definition(uri, &source, position, &laravel)
                });
            drop(laravel);
            if let Some(loc) = laravel_loc {
                return Ok(Some(GotoDefinitionResponse::Scalar(loc)));
            }
            // Bare keyword tokens (`final`, `readonly`, `public`, `string`, ...)
            // are never a resolvable symbol. Bail out before any of the
            // fallbacks below run — in particular the workspace-index lookup
            // near the end of this function matches declarations by bare
            // name across the whole workspace/vendor tree with no kind
            // filter for non-`$`-prefixed queries, so a keyword like `final`
            // or `void` would otherwise "resolve" to any unrelated
            // class/function/property/constant that happens to share its
            // name. See `is_unresolvable_bareword_at` and its use in
            // `handle_references`.
            if let Some(word) = crate::text::word_at_position(&source, position)
                && is_unresolvable_bareword_at(&source, position, &word)
            {
                return Ok(None);
            }
            if let Some(word) = crate::text::word_at_position(&source, position)
                && !word.starts_with('$')
            {
                let analysis = self.cached_analysis_async(uri).await;

                // mir 0.41: ClassReference is recorded on the class token in
                // static calls (Foo::bar), new expressions, instanceof, and
                // type hints. When the cursor sits on a class name, jump
                // directly to the class via PSR-4 using the resolved FQN —
                // more accurate than the workspace index for aliased names.
                if let Some(fqn) = analysis.as_deref().and_then(|a| {
                    let off = crate::text::word_range_at(&source, position)
                        .map(|r| doc.view().byte_of_position(r.start))?;
                    let sym = a.symbol_at(off)?;
                    match &sym.kind {
                        mir_analyzer::ReferenceKind::ClassReference(fqn) => Some(fqn.to_string()),
                        _ => None,
                    }
                }) && let Some(loc) = self.psr4_goto(&fqn).await
                {
                    return Ok(Some(GotoDefinitionResponse::Scalar(loc)));
                }

                // Keep both the short name (workspace-index lookup) and the full
                // FQN Arc (PSR-4 vendor fallback). Arc<str> clone is an atomic
                // increment — no heap allocation on the hot path.
                let resolved_method_target = analysis.as_deref().and_then(|a| {
                    let off = crate::text::word_range_at(&source, position)
                        .map(|r| doc.view().byte_of_position(r.start))?;
                    let sym = a.symbol_at(off)?;
                    match &sym.kind {
                        mir_analyzer::ReferenceKind::MethodCall { class, .. }
                        | mir_analyzer::ReferenceKind::StaticCall { class, .. } => {
                            Some((fqn_short_name(class).to_string(), Arc::clone(class)))
                        }
                        _ => None,
                    }
                });
                if let Some((_, class_fqn_arc)) = resolved_method_target {
                    let wi = self.workspace_index_cached(&mut wi_cache).await;
                    let docs = Arc::clone(&self.docs);
                    let wi_task = Arc::clone(&wi);
                    let class_fqn_task = Arc::clone(&class_fqn_arc);
                    let word_task = word.clone();
                    let found = self
                        .blocking_gated(super::super::debug_gate::GATE_GOTO_DEFINITION, move || {
                            let class_candidates =
                                |short: &str| docs.class_candidates_by_short_name(&wi_task, short);
                            let get_doc = |uri: &Uri| docs.get_doc_salsa(uri);
                            let resolve_class_ref = |fqn: &str| {
                                docs.resolve_class_ref_by_fqn_or_short_name_fallback(&wi_task, fqn)
                            };
                            let loc = find_method_in_class_hierarchy(
                                class_fqn_task.as_ref(),
                                &word_task,
                                &wi_task,
                                &class_candidates,
                                &get_doc,
                                &resolve_class_ref,
                            )?;
                            let refined = docs
                                .get_doc_salsa(&loc.uri)
                                .and_then(|d| {
                                    let range = find_method_range_in_class(
                                        &d,
                                        crate::text::fqn_short_name(class_fqn_task.as_ref()),
                                        &word_task,
                                    )
                                    .or_else(|| find_declaration_range(d.source(), &d, &word_task));
                                    range.map(|range| Location {
                                        uri: loc.uri.clone(),
                                        range,
                                    })
                                })
                                .unwrap_or(loc);
                            Some(refined)
                        })
                        .await
                        .flatten();
                    if let Some(refined) = found {
                        return Ok(Some(GotoDefinitionResponse::Scalar(refined)));
                    }
                    // Fallback: walk the PSR-4 vendor hierarchy for the resolved class.
                    // trim_start_matches is a pointer offset (no allocation).
                    let class_fqn = class_fqn_arc.trim_start_matches('\\');
                    if let Some(loc) = self.psr4_method_goto(class_fqn, &word).await {
                        return Ok(Some(GotoDefinitionResponse::Scalar(loc)));
                    }
                    // May have lazily ingested a vendor file — force the next
                    // fetch to see it.
                    wi_cache = None;
                }

                let resolved_property_target = analysis.as_deref().and_then(|a| {
                    let off = crate::text::word_range_at(&source, position)
                        .map(|r| doc.view().byte_of_position(r.start))?;
                    let sym = a.symbol_at(off)?;
                    match sym.kind.to_name()? {
                        mir_analyzer::Name::Property { class, name } => Some((class, name)),
                        _ => None,
                    }
                });
                if let Some((class_fqn_arc, property_name_arc)) = resolved_property_target {
                    let wi = self.workspace_index_cached(&mut wi_cache).await;
                    let docs = Arc::clone(&self.docs);
                    let wi_task = Arc::clone(&wi);
                    let loc = self
                        .blocking_gated(super::super::debug_gate::GATE_GOTO_DEFINITION, move || {
                            let class_candidates =
                                |short: &str| docs.class_candidates_by_short_name(&wi_task, short);
                            let get_doc = |uri: &Uri| docs.get_doc_salsa(uri);
                            let resolve_class_ref = |fqn: &str| {
                                docs.resolve_class_ref_by_fqn_or_short_name_fallback(&wi_task, fqn)
                            };
                            find_property_in_class_hierarchy(
                                class_fqn_arc.as_ref(),
                                property_name_arc.as_ref(),
                                &wi_task,
                                &class_candidates,
                                &get_doc,
                                &resolve_class_ref,
                            )
                        })
                        .await
                        .flatten();
                    if let Some(loc) = loc {
                        return Ok(Some(GotoDefinitionResponse::Scalar(loc)));
                    }
                }
            }

            let uri_task = uri.clone();
            let source_task = Arc::clone(&source);
            let doc_task = Arc::clone(&doc);
            let local_definition = self
                .blocking_gated(super::super::debug_gate::GATE_GOTO_DEFINITION, move || {
                    crate::navigation::definition::goto_definition(
                        &uri_task,
                        &source_task,
                        &doc_task,
                        &[],
                        position,
                    )
                })
                .await
                .flatten();
            if let Some(loc) = local_definition {
                return Ok(Some(GotoDefinitionResponse::Scalar(loc)));
            }
            if let Some(line_text) = source.lines().nth(position.line as usize)
                && let Some(word) = crate::text::word_at_position(&source, position)
                && let Some(receiver) = crate::hover::extract_receiver_var_before_cursor(
                    line_text,
                    position.character as usize,
                )
            {
                let class_name = if receiver == "$this" {
                    enclosing_class_at(&source, &doc, position)
                } else {
                    let analysis = self.cached_analysis_async(uri).await;
                    analysis.as_deref().and_then(|a| {
                        let off = receiver_var_offset(&doc, line_text, position, &receiver)?;
                        crate::types::type_query::type_at_offset(a, off)
                            .and_then(crate::types::type_query::primary_class_name)
                    })
                };
                if let Some(cls) = class_name {
                    let first_cls = cls.split('|').next().unwrap_or(&cls).to_owned();
                    let wi2 = self.workspace_index_cached(&mut wi_cache).await;
                    let docs = Arc::clone(&self.docs);
                    let wi_task = Arc::clone(&wi2);
                    let first_cls_task = first_cls.clone();
                    let word_task = word.clone();
                    let found = self
                        .blocking_gated(super::super::debug_gate::GATE_GOTO_DEFINITION, move || {
                            let class_candidates =
                                |short: &str| docs.class_candidates_by_short_name(&wi_task, short);
                            let get_doc = |uri: &Uri| docs.get_doc_salsa(uri);
                            let resolve_class_ref = |fqn: &str| {
                                docs.resolve_class_ref_by_fqn_or_short_name_fallback(&wi_task, fqn)
                            };
                            let loc = find_method_in_class_hierarchy(
                                &first_cls_task,
                                &word_task,
                                &wi_task,
                                &class_candidates,
                                &get_doc,
                                &resolve_class_ref,
                            )?;
                            let refined = docs
                                .get_doc_salsa(&loc.uri)
                                .and_then(|doc| {
                                    find_declaration_range(doc.source(), &doc, &word_task).map(
                                        |range| Location {
                                            uri: loc.uri.clone(),
                                            range,
                                        },
                                    )
                                })
                                .unwrap_or(loc);
                            Some(refined)
                        })
                        .await
                        .flatten();
                    if let Some(refined) = found {
                        return Ok(Some(GotoDefinitionResponse::Scalar(refined)));
                    }
                    // Fallback: resolve the class FQN via the workspace index and
                    // walk the PSR-4 vendor hierarchy starting from there.
                    let class_fqn = self
                        .docs
                        .resolve_class_ref_by_fqn_or_short_name_fallback(&wi2, &first_cls)
                        .and_then(|cr| {
                            wi2.at(cr)
                                .map(|(_, cls)| cls.fqn.trim_start_matches('\\').to_owned())
                        })
                        .unwrap_or_else(|| first_cls.clone());
                    if let Some(loc) = self.psr4_method_goto(&class_fqn, &word).await {
                        return Ok(Some(GotoDefinitionResponse::Scalar(loc)));
                    }
                    // May have lazily ingested a vendor file — force the next
                    // fetch to see it.
                    wi_cache = None;
                }
            }

            let wi = self.workspace_index_cached(&mut wi_cache).await;
            if let Some(word) = crate::text::word_at_position(&source, position) {
                let docs = Arc::clone(&self.docs);
                let wi_task = Arc::clone(&wi);
                let uri_task = uri.clone();
                let found = self
                    .blocking_gated(super::super::debug_gate::GATE_GOTO_DEFINITION, move || {
                        let loc = docs.find_declaration(&wi_task, &word, Some(&uri_task))?;
                        let refined = docs
                            .get_doc_salsa(&loc.uri)
                            .and_then(|doc| {
                                find_declaration_range(doc.source(), &doc, &word).map(|range| {
                                    Location {
                                        uri: loc.uri.clone(),
                                        range,
                                    }
                                })
                            })
                            .unwrap_or(loc);
                        Some(refined)
                    })
                    .await
                    .flatten();
                if let Some(refined) = found {
                    return Ok(Some(GotoDefinitionResponse::Scalar(refined)));
                }
            }

            if let Some(word) = word_at_position(&source, position)
                && word.contains('\\')
            {
                let imports = crate::navigation::references::collect_class_imports(&doc);
                let expanded = expand_alias_prefix(&word, &imports);
                if let Some(loc) = self.psr4_goto(&expanded).await {
                    return Ok(Some(GotoDefinitionResponse::Scalar(loc)));
                }
            }

            // Resolve `use Foo\Bar as Alias` → navigate to Foo\Bar.
            // Handles cursor on the alias name in `implements Alias` or `extends Alias`
            // where the alias was introduced by a `use … as Alias` statement in this file.
            if let Some(word) = word_at_position(&source, position)
                && !word.contains('\\')
            {
                let imports = crate::navigation::references::collect_class_imports(&doc);
                if let Some(fqn) = imports.get(&word as &str)
                    && let Some(loc) = self.psr4_goto(fqn).await
                {
                    return Ok(Some(GotoDefinitionResponse::Scalar(loc)));
                }
                // PSR-0 fallback: bare class names with underscores (e.g. `Acme_Client`)
                // are not in the workspace index when vendor is excluded. Try PSR-0 resolution.
                if let Some(word) = word_at_position(&source, position)
                    && let Some(loc) = self.psr4_goto(&word).await
                {
                    return Ok(Some(GotoDefinitionResponse::Scalar(loc)));
                }
            }

            Ok(None)
        })
        .await
    }

    pub(crate) async fn handle_references(
        &self,
        params: ReferenceParams,
    ) -> Result<Option<Vec<Location>>> {
        guard_async_result("references", async move {
            let uri = &params.text_document_position.text_document.uri;
            let position = params.text_document_position.position;
            let source = self.get_open_text(uri).unwrap_or_default();
            let include_declaration = params.context.include_declaration;

            // Laravel string-key definition sites (`.env` entry, config array
            // key, view template start, translation key, route `->name(...)`)
            // aren't `word_at_position` matches, so this must run before that
            // check. `resolve_definition_key` checks `is_laravel` first (one
            // atomic load + bool check) so non-Laravel workspaces never pay
            // for the reverse-lookup scan inside it.
            let laravel = self.laravel.load();
            let laravel_def = crate::laravel::resolve_definition_key(uri, position, &laravel);
            drop(laravel);
            if let Some((names, key, def_location)) = laravel_def {
                let mut locations = if include_declaration {
                    vec![def_location]
                } else {
                    vec![]
                };
                // Every workspace file's cached text gets the substring
                // prefilter below (and a parse on a hit), so this runs on the
                // blocking pool like every other workspace-scanning path in
                // this handler — never sequentially on the tokio worker.
                let docs = Arc::clone(&self.docs);
                let found = tokio::task::spawn_blocking(move || {
                    let mut found = Vec::new();
                    docs.with_all_indexes(|all_indexes| {
                        for (file_uri, _) in all_indexes {
                            // Text prefilter: find_call_sites only matches
                            // string literals whose parsed content equals
                            // `key` exactly, so a raw substring miss
                            // guarantees no match — skips parsing every file
                            // that doesn't mention it at all.
                            if !docs
                                .source_text(file_uri)
                                .is_some_and(|t| t.contains(key.as_str()))
                            {
                                continue;
                            }
                            let Some(doc) = docs.get_doc_salsa(file_uri) else {
                                continue;
                            };
                            for range in crate::laravel::find_call_sites(&doc, names, &key) {
                                found.push(Location {
                                    uri: file_uri.clone(),
                                    range,
                                });
                            }
                        }
                    });
                    found
                })
                .await
                .unwrap_or_default();
                locations.extend(found);
                return Ok(Some(locations));
            }

            let word = match word_at_position(&source, position) {
                Some(w) => w,
                None => return Ok(None),
            };

            // Bare keyword tokens (`final`, `readonly`, `abstract`, ...) and
            // documentation-only PHPDoc tokens (`@param`'s tag name, a
            // `@template` parameter name, a doc `$var` name, ...) are never
            // a resolvable symbol. Bail out before mir's per-file analysis
            // runs: its declaration span for the entity the keyword modifies
            // (e.g. a class) can otherwise map the offset to that entity's
            // real symbol, triggering a full reference search from a token
            // that was never meant to be searchable.
            if is_unresolvable_bareword_at(&source, position, &word) {
                return Ok(None);
            }

            if word == "__construct"
                && let Some(doc) = self.get_doc(uri)
            {
                // Try declaration site first. `parent::` is compile-time resolved
                // in PHP — it always names the literal `extends` class, never
                // subject to late static binding — so a `parent::__construct()`
                // call site must resolve to that parent, not the enclosing
                // (child) class. Only fall back to the enclosing-class heuristic
                // when the parent can't be resolved (e.g. an external/vendor
                // base class not present in the workspace index).
                let decl_class =
                    class_name_at_construct_decl(doc.source(), &doc.program().stmts, position);
                let on_call_site = decl_class.is_none();
                let is_parent_call_site = on_call_site
                    && class_before_double_colon(&source, position).as_deref() == Some("parent");
                let class_name = if let Some(decl_class) = decl_class {
                    Some(decl_class)
                } else if is_parent_call_site {
                    let wi = self.workspace_index_async().await;
                    let imports = self.file_imports(uri);
                    resolve_parent_construct_class(&doc, position, &wi, &self.docs, &imports)
                        .or_else(|| enclosing_class_fqn_at(doc.source(), &doc, position))
                } else {
                    enclosing_class_fqn_at(doc.source(), &doc, position)
                };
                if let Some(class_name) = class_name {
                    // When cursor is on a call site (not the `function __construct`
                    // declaration), exclude the cursor span from results — it points
                    // to the `parent::__construct()` text, not to the declaration.
                    let incl_decl = include_declaration && !on_call_site;
                    // Instantiation sites (`new Short(...)`) always name the
                    // class's short name; mir records them under
                    // `meth:{fqcn}::__construct`.
                    let fqn = if class_name.contains('\\') {
                        class_name.trim_start_matches('\\').to_string()
                    } else {
                        let imports = self.file_imports(uri);
                        crate::navigation::moniker::resolve_fqn(&doc, &class_name, &imports)
                            .trim_start_matches('\\')
                            .to_string()
                    };
                    let sym = mir_analyzer::Name::method(fqn.as_str(), "__construct");
                    let files: Vec<Arc<str>> = self.docs.reference_candidate_files(&sym);
                    let docs = Arc::clone(&self.docs);
                    let locations = tokio::task::spawn_blocking(move || {
                        let (_interactive, cancel_rev) = docs.settled_write_rev_guard();
                        let mut locs: Vec<Location> = docs
                            .indexed_references(&sym, &files, incl_decl, Some(cancel_rev))
                            .into_iter()
                            .filter_map(session_tuple_to_location)
                            .collect();
                        dedup_ref_locations(&mut locs);
                        locs
                    })
                    .await
                    .unwrap_or_default();
                    return Ok((!locations.is_empty()).then_some(locations));
                }
                // Cannot determine the owning class — return empty rather than
                // falling through to the unscoped method-reference path.
                return Ok(None);
            }

            // Variables: scope-aware search within the enclosing function/method.
            // The general-purpose reference walker only matches identifiers, not
            // `ExprKind::Variable`, so variables would otherwise return nothing.
            // Skip this path for promoted properties — they need the general
            // property-reference search (which also finds `$this->name` accesses).
            // Skip also when var_spans is empty (class/static property declarations,
            // top-level declarations) so the general path can handle them.
            if word.starts_with('$')
                && let Some(doc) = self.get_doc(uri)
            {
                let uri = uri.clone();
                let local_word = word.clone();
                let local_locations = self
                    .blocking_gated(
                        super::super::debug_gate::GATE_REFERENCES_VARIABLE,
                        move || {
                            let is_promoted = promoted_property_at_cursor(
                                doc.source(),
                                &doc.program().stmts,
                                position,
                            )
                            .is_some();
                            if is_promoted {
                                return None;
                            }

                            let bare = local_word.trim_start_matches('$');
                            let byte_off = doc.view().byte_of_position(position) as usize;
                            let mut var_spans = Vec::new();
                            collect_var_refs_in_scope(
                                &doc.program().stmts,
                                bare,
                                byte_off,
                                &mut var_spans,
                            );
                            if var_spans.is_empty() {
                                return None;
                            }

                            let name_with_sigil = format!("${bare}");
                            let name_utf16_len = utf16_code_units(&name_with_sigil);
                            let sv = doc.view();
                            let src = doc.source();
                            let precise_starts: Vec<u32> = var_spans
                                .iter()
                                .map(|(span, _kind)| {
                                    // Param spans include type annotations; narrow to $var_name.
                                    crate::document::ast::str_offset_in_range(
                                        src,
                                        *span,
                                        &name_with_sigil,
                                    )
                                    .unwrap_or(span.start)
                                })
                                .collect();
                            let starts = sv.positions_of_offsets(&precise_starts);
                            let locations: Vec<Location> = starts
                                .into_iter()
                                .map(|start| Location {
                                    uri: uri.clone(),
                                    range: Range {
                                        start,
                                        end: Position {
                                            line: start.line,
                                            character: start.character + name_utf16_len,
                                        },
                                    },
                                })
                                .collect();
                            Some(locations)
                        },
                    )
                    .await
                    .flatten();
                if let Some(locations) = local_locations {
                    return Ok(Some(locations));
                }
            }
            // Fall through to the general reference path for:
            // - promoted properties (need cross-method $this->prop search)
            // - class/static property declarations (var_spans empty)
            // - any other $word the scope walker didn't find

            let doc_opt = self.get_doc(uri);

            // Usage-site cursor: mir's per-file analysis already resolved the
            // symbol under the cursor (receiver types, aliases, namespaces) —
            // its `ReferenceKind` maps 1:1 onto the index key. Waits and
            // retries once if mir hasn't resolved it yet (a companion file
            // this reference depends on, e.g. a `use` import's declaring
            // file, can still be settling in the background).
            let usage_symbol = self
                .resolve_usage_symbol_with_retry(uri, doc_opt.as_ref(), &source, position)
                .await;

            // Declaration-site cursor (or no analysis): classify the cursor
            // context and resolve the owner/target FQN.
            let (word, kind, constant_owner) =
                resolve_reference_symbol(doc_opt.as_ref(), &source, position, word);
            let symbol = match usage_symbol {
                Some(sym) => sym,
                None => {
                    let target_fqn = self.resolve_reference_target_fqn(
                        uri,
                        doc_opt.as_ref(),
                        &word,
                        kind,
                        position,
                        constant_owner.clone(),
                    );
                    match build_mir_symbol(
                        &word,
                        kind,
                        target_fqn.as_deref(),
                        constant_owner.is_some(),
                    ) {
                        Some(sym) => sym,
                        None => return Ok(None),
                    }
                }
            };

            // Candidate scope: visibility narrowing for private/protected
            // methods, else the whole workspace — mir gates never-committed
            // candidates on a symbol-name text mention internally.
            let mut files: Vec<Arc<str>> = self.docs.reference_candidate_files(&symbol);
            // The requesting file's own text is what the cursor's `use`
            // import (or any other mention) lives in, so it must always be
            // in scope regardless of what the narrowing above computed —
            // a companion file opened just before this request (the common
            // `use`-import shape: declaring file opened right after the
            // importing one) can make the reachability narrowing's mention/
            // posting data for *this* file briefly lag, wrongly dropping it.
            let uri_str: Arc<str> = Arc::from(uri.as_str());
            if !files.iter().any(|f| f.as_ref() == uri_str.as_ref()) {
                files.push(uri_str);
            }
            let wants_use_imports = matches!(
                symbol,
                mir_analyzer::Name::Class(_)
                    | mir_analyzer::Name::Function(_)
                    | mir_analyzer::Name::GlobalConstant(_)
            );

            // Priority streaming: if the client asked for partial results,
            // analyze the subset of candidates that already mention the
            // owner's short name first and stream those ahead of the full
            // (authoritative) query below — those files are the likely hits
            // when the owner name is a common word shared by many unrelated
            // classes, so this is where the user-visible latency lives.
            // Skipped entirely (no extra mir call) when no token is present,
            // and also when the candidate scope is already narrowed (private/
            // protected/static methods): `files` is then already the minimal
            // necessary set, so partitioning it further would just re-scan
            // those same bytes for no streaming benefit.
            if let Some(token) = params.partial_result_params.partial_result_token.clone() {
                let owner_short = match &symbol {
                    mir_analyzer::Name::Method { class, name }
                        if !class.is_empty()
                            && !self.docs.method_scope_is_narrowed(class, name) =>
                    {
                        Some(fqn_short_name(class).to_string())
                    }
                    _ => None,
                };
                if let Some(owner_short) = owner_short {
                    let docs = Arc::clone(&self.docs);
                    let all_files = files.clone();
                    // The owner-mention partition may scan candidate texts
                    // (cold files only — warm ones answer from mir's mention
                    // cache) — do it on the blocking pool, never on the
                    // tokio worker. mir parallelizes internally. Once split,
                    // query the priority subset and the remainder separately
                    // so the authoritative response doesn't pay the priority
                    // files twice.
                    //
                    // Must settle first, same as the `indexed_references`
                    // calls below: mir's mention scan silently drops any file
                    // not yet registered as a salsa source (a background
                    // write still in flight), which would misclassify a
                    // genuine owner mention as absent under load instead of
                    // just seeing it late.
                    let (priority_files, remainder_files) =
                        tokio::task::spawn_blocking(move || {
                            let _interactive = docs.settled_write_rev_guard();
                            let priority_files: Vec<Arc<str>> =
                                docs.files_mentioning_short_name(&all_files, &owner_short);
                            let priority_set: std::collections::HashSet<&str> =
                                priority_files.iter().map(|f| f.as_ref()).collect();
                            let remainder_files: Vec<Arc<str>> = all_files
                                .into_iter()
                                .filter(|f| !priority_set.contains(f.as_ref()))
                                .collect();
                            (priority_files, remainder_files)
                        })
                        .await
                        .unwrap_or_default();

                    if !priority_files.is_empty() {
                        let priority_docs = Arc::clone(&self.docs);
                        let priority_sym = symbol.clone();
                        let mut locations = tokio::task::spawn_blocking(move || {
                            let (_interactive, cancel_rev) =
                                priority_docs.settled_write_rev_guard();
                            let mut locs: Vec<Location> = priority_docs
                                .indexed_references(
                                    &priority_sym,
                                    &priority_files,
                                    include_declaration,
                                    Some(cancel_rev),
                                )
                                .into_iter()
                                .filter_map(session_tuple_to_location)
                                .collect();
                            dedup_ref_locations(&mut locs);
                            locs
                        })
                        .await
                        .unwrap_or_default();
                        if !locations.is_empty() {
                            super::super::send_references_partial_result(
                                &self.client,
                                token,
                                locations.clone(),
                            )
                            .await;
                        }
                        if !remainder_files.is_empty() {
                            let remainder_docs = Arc::clone(&self.docs);
                            let remainder_sym = symbol.clone();
                            locations.extend(
                                tokio::task::spawn_blocking(move || {
                                    let (_interactive, cancel_rev) =
                                        remainder_docs.settled_write_rev_guard();
                                    let mut locs: Vec<Location> = remainder_docs
                                        .indexed_references(
                                            &remainder_sym,
                                            &remainder_files,
                                            include_declaration,
                                            Some(cancel_rev),
                                        )
                                        .into_iter()
                                        .filter_map(session_tuple_to_location)
                                        .collect();
                                    dedup_ref_locations(&mut locs);
                                    locs
                                })
                                .await
                                .unwrap_or_default(),
                            );
                            dedup_ref_locations(&mut locations);
                        }
                        if include_declaration {
                            locations.retain(|loc| location_starts_on_symbol(&self.docs, loc));
                            locations.extend(self.workspace_decl_locations(&symbol, &word).await);
                            dedup_ref_locations(&mut locations);
                        }
                        return Ok((!locations.is_empty()).then_some(locations));
                    }
                }
            }

            // Declaration coverage comes from mir's definitions index (the
            // `include_declaration` flag below) — never from the raw cursor
            // span, which on a `use` import line is not a reference at all.
            // Always the full candidate set, run unconditionally: this is the
            // authoritative response, identical whether or not a priority
            // batch was streamed above — `analyze_file`'s LRU makes
            // re-analyzing the priority subset here nearly free.
            let mut locations = self
                .indexed_references_for_symbol(
                    &symbol,
                    &files,
                    include_declaration,
                    wants_use_imports,
                )
                .await;
            if locations.is_empty() {
                // A candidate admitted by the cold-file text-mention gate but
                // never analyzed before this query (e.g. an edit that just
                // introduced the only cross-file usage) can still be
                // committing its class relationship into mir's index — that
                // commit can itself advance mir's own generation counter
                // mid-query, which is exactly the condition mir's result
                // cache treats as too fresh to cache. Same settle-then-retry
                // shape as `resolve_usage_symbol_with_retry`; a genuinely
                // empty result just repeats (cheap: mir's cache serves the
                // repeat from memo).
                if !self.docs.is_index_ready() {
                    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(50);
                    let mut rev = self.docs.write_rev();
                    loop {
                        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
                        let now = self.docs.write_rev();
                        let quiet = now == rev;
                        rev = now;
                        if quiet || std::time::Instant::now() >= deadline {
                            break;
                        }
                    }
                }
                locations = self
                    .indexed_references_for_symbol(
                        &symbol,
                        &files,
                        include_declaration,
                        wants_use_imports,
                    )
                    .await;
            }
            if include_declaration {
                locations.retain(|loc| location_starts_on_symbol(&self.docs, loc));
                locations.extend(self.workspace_decl_locations(&symbol, &word).await);
                dedup_ref_locations(&mut locations);
            }

            Ok((!locations.is_empty()).then_some(locations))
        })
        .await
    }

    /// Rename via mir's inverted indexes: resolve the symbol under the cursor
    /// exactly like the references handler, collect its posting-list sites
    /// *plus* the declaration name token (`include_declaration`) and — for
    /// importable symbols — the `use:` import lines, then narrow each span to
    /// the renamed token (qualified names and import items cover the whole
    /// written path).
    ///
    /// Variables never reach this: their rename is scope-bound to one
    /// document and stays on the AST scope walker.
    pub(crate) async fn indexed_rename(
        &self,
        uri: &Uri,
        position: Position,
        new_name: &str,
    ) -> Option<WorkspaceEdit> {
        let source = self.get_open_text(uri).unwrap_or_default();
        let word = word_at_position(&source, position)?;
        if is_unresolvable_bareword_at(&source, position, &word) {
            return None;
        }
        let doc_opt = self.get_doc(uri);

        // Usage-site cursor: mir's per-file analysis already resolved the
        // symbol (receiver types, aliases, namespaces). Waits and retries
        // once if mir hasn't resolved it yet — see
        // `resolve_usage_symbol_with_retry`.
        let usage_symbol = self
            .resolve_usage_symbol_with_retry(uri, doc_opt.as_ref(), &source, position)
            .await;

        // Declaration-site cursor (or no analysis): classify the cursor
        // context and resolve the owner/target FQN.
        let (word, kind, constant_owner) =
            resolve_reference_symbol(doc_opt.as_ref(), &source, position, word);
        let symbol = match usage_symbol {
            Some(sym) => sym,
            None => {
                let target_fqn = self.resolve_reference_target_fqn(
                    uri,
                    doc_opt.as_ref(),
                    &word,
                    kind,
                    position,
                    constant_owner.clone(),
                );
                build_mir_symbol(&word, kind, target_fqn.as_deref(), constant_owner.is_some())?
            }
        };

        // Candidate scope: same shape as the references handler — visibility
        // narrowing for private/protected methods, else the whole workspace
        // (mir gates never-committed candidates internally).
        let files: Vec<Arc<str>> = self.docs.reference_candidate_files(&symbol);

        // `use` imports are recorded under separate `use:` keys so plain
        // find-references stays blind to them; a rename must edit them too.
        let wants_use_imports = matches!(
            symbol,
            mir_analyzer::Name::Class(_)
                | mir_analyzer::Name::Function(_)
                | mir_analyzer::Name::GlobalConstant(_)
        );
        // Class/function declaration tokens come from the salsa workspace
        // index, which matches names case-sensitively — mir's declaration
        // lookup follows PHP's case-insensitive dispatch and would return a
        // declaration differing only in case. Member declarations stay on
        // mir's hierarchy-aware lookup.
        let mir_include_decl = !matches!(
            symbol,
            mir_analyzer::Name::Class(_) | mir_analyzer::Name::Function(_)
        );
        let docs = Arc::clone(&self.docs);
        let sym = symbol.clone();
        let mut locations = tokio::task::spawn_blocking(move || {
            let (_interactive, cancel_rev) = docs.settled_write_rev_guard();
            let mut locs: Vec<Location> = docs
                .indexed_references(&sym, &files, mir_include_decl, Some(cancel_rev))
                .into_iter()
                .filter_map(session_tuple_to_location)
                .collect();
            if wants_use_imports {
                // The freshness pass above committed the candidates, so this
                // read-only lookup observes their current `use:` postings.
                locs.extend(
                    docs.indexed_use_imports(&sym, &files)
                        .into_iter()
                        .filter_map(session_tuple_to_location),
                );
            }
            dedup_ref_locations(&mut locs);
            locs
        })
        .await
        .unwrap_or_default();

        if !mir_include_decl {
            locations.extend(self.workspace_decl_locations(&symbol, &word).await);
            dedup_ref_locations(&mut locations);
        }

        let mut changes: std::collections::HashMap<Uri, Vec<TextEdit>> =
            std::collections::HashMap::new();
        let mut doc_cache: std::collections::HashMap<
            Uri,
            Option<Arc<crate::document::ast::ParsedDoc>>,
        > = std::collections::HashMap::new();
        for loc in locations {
            let ndoc = doc_cache
                .entry(loc.uri.clone())
                .or_insert_with(|| self.docs.get_doc_salsa(&loc.uri));
            let Some(doc) = ndoc else { continue };
            let Some(range) =
                crate::editing::rename::narrow_range_to_word(doc.source(), loc.range, &word)
            else {
                continue;
            };
            changes.entry(loc.uri).or_default().push(TextEdit {
                range,
                new_text: new_name.to_string(),
            });
        }
        for edits in changes.values_mut() {
            crate::editing::rename::sort_and_dedup_edits(edits);
        }
        Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        })
    }

    /// Run mir's indexed-references query for `symbol` over `files` plus,
    /// when `wants_use_imports`, the separate `use:` postings — the shared
    /// body behind `handle_references`'s authoritative (and, on an empty
    /// first attempt, retried) lookup.
    async fn indexed_references_for_symbol(
        &self,
        symbol: &mir_analyzer::Name,
        files: &[Arc<str>],
        include_declaration: bool,
        wants_use_imports: bool,
    ) -> Vec<Location> {
        let docs = Arc::clone(&self.docs);
        let symbol = symbol.clone();
        let files = files.to_vec();
        tokio::task::spawn_blocking(move || {
            // Pause the background scan and snapshot a settled revision so
            // only a genuine user edit cancels the search.
            let (_interactive, cancel_rev) = docs.settled_write_rev_guard();
            let mut locs: Vec<Location> = docs
                .indexed_references(&symbol, &files, include_declaration, Some(cancel_rev))
                .into_iter()
                .filter_map(session_tuple_to_location)
                .collect();
            if wants_use_imports {
                // The freshness pass above commits candidate analyses, so
                // the separate `use:` postings are current before this
                // read-only lookup appends them.
                locs.extend(
                    docs.indexed_use_imports(&symbol, &files)
                        .into_iter()
                        .filter_map(session_tuple_to_location),
                );
            }
            dedup_ref_locations(&mut locs);
            locs
        })
        .await
        .unwrap_or_default()
    }

    /// Mir's own per-file resolution of the symbol under the cursor
    /// (receiver types, aliases, namespaces) — its `ReferenceKind` maps 1:1
    /// onto the index key. `None` either because the cursor is on a
    /// declaration (mir only resolves usages) or because mir hasn't yet
    /// analyzed a companion file this reference depends on.
    async fn resolve_usage_symbol(
        &self,
        uri: &Uri,
        doc_opt: Option<&Arc<crate::document::ast::ParsedDoc>>,
        source: &str,
        position: Position,
    ) -> Option<mir_analyzer::Name> {
        let analysis = self.cached_analysis_async(uri).await;
        analysis.as_deref().and_then(|a| {
            let doc = doc_opt?;
            let off = crate::text::word_range_at(source, position)
                .map(|r| doc.view().byte_of_position(r.start))?;
            a.symbol_at(off).and_then(|s| s.kind.to_name())
        })
    }

    /// [`Self::resolve_usage_symbol`], but when the first attempt comes back
    /// empty, waits for the write revision to quiesce and retries once
    /// before giving up. A companion file a usage-site reference depends on
    /// (e.g. a `use` import's declaring file, opened right before this
    /// request in the same batch) can still be settling in the background
    /// when the first attempt runs — trust mir's own resolution over the
    /// AST-heuristic FQN fallback (`resolve_reference_target_fqn`) whenever
    /// it can actually produce one, rather than falling through to that
    /// fallback the moment mir is merely running behind.
    async fn resolve_usage_symbol_with_retry(
        &self,
        uri: &Uri,
        doc_opt: Option<&Arc<crate::document::ast::ParsedDoc>>,
        source: &str,
        position: Position,
    ) -> Option<mir_analyzer::Name> {
        let first = self
            .resolve_usage_symbol(uri, doc_opt, source, position)
            .await;
        if first.is_some() {
            return first;
        }
        // Test-only hold point: lets a test force a companion declaring file
        // to open between this empty attempt and the settle-wait/retry
        // below, proving the retry actually observes it. No-op outside test
        // builds.
        self.blocking_gated(super::super::debug_gate::GATE_USAGE_SYMBOL_RETRY, || ())
            .await;
        if !self.docs.is_index_ready() {
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(50);
            let mut rev = self.docs.write_rev();
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(2)).await;
                let now = self.docs.write_rev();
                let quiet = now == rev;
                rev = now;
                if quiet || std::time::Instant::now() >= deadline {
                    break;
                }
            }
        }
        self.resolve_usage_symbol(uri, doc_opt, source, position)
            .await
    }

    /// Declaration name tokens for a class or function symbol from the salsa
    /// workspace index — matched and ranged by `word`'s own short name (a
    /// no-op shortening when `word` is already bare), never by the resolved
    /// FQN's short name: mir's own resolution follows PHP's case-insensitive
    /// dispatch, so it can differ in case from what the user actually typed.
    /// Prefers declarations whose FQN matches the symbol; falls back to
    /// same-named declarations when none does.
    ///
    /// A companion file opened moments earlier (e.g. the declaring file for
    /// a `use` import, opened right before this request in the same batch)
    /// can race the workspace-index snapshot this reads: [`Self::workspace_decl_locations_once`]
    /// comes back empty on the first try, then non-empty on a fresh re-fetch,
    /// which is only explainable by that snapshot having briefly lagged
    /// behind the file set. An immediate re-read only wins that race by luck
    /// (whatever scheduling happens to land between the two calls), so under
    /// real concurrent load it can still lose — wait for the write revision
    /// to quiesce first, the same freshness condition `settled_write_rev_guard`
    /// enforces for the mir query paths, before retrying.
    async fn workspace_decl_locations(
        &self,
        symbol: &mir_analyzer::Name,
        word: &str,
    ) -> Vec<Location> {
        let first = self.workspace_decl_locations_once(symbol, word);
        if !first.is_empty() {
            return first;
        }
        // Test-only hold point: lets a test force a companion declaring file
        // to open between this empty attempt and the settle-wait/retry below,
        // proving the retry actually observes it. No-op outside test builds.
        self.blocking_gated(
            super::super::debug_gate::GATE_WORKSPACE_DECL_LOCATIONS_RETRY,
            || (),
        )
        .await;
        if !self.docs.is_index_ready() {
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(50);
            let mut rev = self.docs.write_rev();
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(2)).await;
                let now = self.docs.write_rev();
                let quiet = now == rev;
                rev = now;
                if quiet || std::time::Instant::now() >= deadline {
                    break;
                }
            }
        }
        self.workspace_decl_locations_once(symbol, word)
    }

    fn workspace_decl_locations_once(
        &self,
        symbol: &mir_analyzer::Name,
        word: &str,
    ) -> Vec<Location> {
        let ws = self.docs.get_workspace_index_salsa();
        // The cursor's own `word` is whatever's under it as literally typed —
        // on a qualified path (e.g. a `use` import's `App\Services\Logger`)
        // that's the whole path, not the declared short name a
        // workspace-index class/function is keyed and ranged by. Shorten
        // `word` itself (a no-op when it's already a bare name) rather than
        // using the resolved FQN's short name: mir's own FQN resolution
        // follows PHP's case-insensitive dispatch and can differ in case
        // from what the user actually typed, which would silently break
        // rename's case-sensitive declaration matching below.
        let short = fqn_short_name(word);
        let name_range = |line: u32, ch: u32| tower_lsp_server::ls_types::Range {
            start: Position {
                line,
                character: ch,
            },
            end: Position {
                line,
                character: ch + utf16_code_units(short),
            },
        };
        let mut exact = Vec::new();
        let mut by_name = Vec::new();
        match symbol {
            mir_analyzer::Name::Class(fqn) => {
                let target = fqn.trim_start_matches('\\');
                for r in &self.docs.class_candidates_by_short_name(&ws, short) {
                    let Some((uri, cls)) = ws.at(*r) else {
                        continue;
                    };
                    if cls.name.as_ref() != short {
                        continue;
                    }
                    let loc = Location {
                        uri: uri.clone(),
                        range: name_range(cls.start_line, cls.name_char),
                    };
                    if cls.fqn.trim_start_matches('\\') == target {
                        exact.push(loc);
                    } else {
                        by_name.push(loc);
                    }
                }
            }
            mir_analyzer::Name::Function(fqn) => {
                let target = fqn.trim_start_matches('\\');
                for (uri, idx) in &ws.files {
                    for f in &idx.functions {
                        if f.name.as_ref() != short {
                            continue;
                        }
                        let loc = Location {
                            uri: uri.clone(),
                            range: name_range(f.start_line, f.name_char),
                        };
                        if f.fqn.trim_start_matches('\\') == target {
                            exact.push(loc);
                        } else {
                            by_name.push(loc);
                        }
                    }
                }
            }
            _ => {}
        }
        if !exact.is_empty() {
            exact
        } else if word.contains('\\') {
            // A qualified cursor word already names its own namespace, so a
            // same-short-name declaration elsewhere that its FQN doesn't
            // match is a different symbol, not a stale-index near-miss —
            // e.g. `use App\Logger;` referring to nothing real must not
            // spuriously match an unrelated global `class Logger {}`.
            // Bare-word cursors carry no such namespace claim, so their
            // by-name fallback stands.
            Vec::new()
        } else {
            by_name
        }
    }

    pub(crate) async fn handle_linked_editing_range(
        &self,
        params: LinkedEditingRangeParams,
    ) -> Result<Option<LinkedEditingRanges>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let source = self.get_open_text(uri).unwrap_or_default();
        let doc = match self.get_doc(uri) {
            Some(d) => d,
            None => return Ok(None),
        };
        // The highlight/class-scoping walks below are CPU-bound over the
        // whole document; keep them off the async runtime worker (the
        // same work `document_highlight` already runs via spawn_blocking).
        Ok(self
            .blocking_gated(
                super::super::debug_gate::GATE_LINKED_EDITING_RANGE,
                move || {
                    let word = crate::text::word_at_position(&source, position)?;
                    let is_variable = word.starts_with('$');
                    let cursor_word_range = crate::text::word_range_at(&source, position)?;

                    let highlights = document_highlights(&source, &doc, position);
                    if highlights.is_empty() {
                        return None;
                    }

                    if !highlights.iter().any(|h| h.range == cursor_word_range) {
                        return None;
                    }

                    let scope_to_class = !is_variable
                        && crate::types::type_map::enclosing_class_at(&source, &doc, position)
                            .as_deref()
                            != Some(word.as_str());
                    let other_class_ranges: Vec<Range> = if scope_to_class {
                        let cursor_class =
                            crate::types::type_map::enclosing_class_range_at(&doc, position);
                        crate::types::type_map::collect_all_class_ranges(&doc)
                            .into_iter()
                            .filter(|r| Some(*r) != cursor_class)
                            .collect()
                    } else {
                        Vec::new()
                    };
                    let ranges: Vec<Range> = highlights
                        .into_iter()
                        .map(|h| h.range)
                        .filter(|r| !other_class_ranges.iter().any(|ocr| range_within(*r, *ocr)))
                        .collect();
                    if ranges.is_empty() {
                        return None;
                    }

                    let word_pattern = if is_variable {
                        "\\$[a-zA-Z_\\u00A0-\\uFFFF][a-zA-Z0-9_\\u00A0-\\uFFFF]*".to_string()
                    } else {
                        "[a-zA-Z_\\u00A0-\\uFFFF][a-zA-Z0-9_\\u00A0-\\uFFFF]*".to_string()
                    };
                    Some(LinkedEditingRanges {
                        ranges,
                        word_pattern: Some(word_pattern),
                    })
                },
            )
            .await
            .unwrap_or_default())
    }
}

fn location_starts_on_symbol(
    docs: &crate::document::document_store::DocumentStore,
    loc: &Location,
) -> bool {
    let Some(source) = docs.source_text(&loc.uri) else {
        return true;
    };
    let Some(raw_line) = source.split('\n').nth(loc.range.start.line as usize) else {
        return true;
    };
    let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
    let start = utf16_offset_to_byte(line, loc.range.start.character as usize);
    let Some(text) = line.get(start..) else {
        return true;
    };
    text.chars().next().is_some_and(|c| {
        c.is_alphabetic() || c == '_' || c.is_ascii_digit() || c == '\\' || c == '$' || c == '&'
    })
}

fn expand_alias_prefix(word: &str, imports: &std::collections::HashMap<String, String>) -> String {
    if let Some((first, rest)) = word.split_once('\\')
        && let Some(ns_prefix) = imports.get(first)
    {
        return format!("{}\\{}", ns_prefix, rest);
    }
    word.to_string()
}

/// Resolve a `parent::__construct()` call site to the FQN of the class named
/// in the enclosing class's `extends` clause. Looks up the `extends` name
/// (same-file, as written in source) and resolves it the same way PHP
/// itself would: `use`-import match, else the declaring file's own
/// namespace — never a same-short-name guess across the workspace. A bare
/// `extends Base` has exactly one meaning (PHP has no "which same-named
/// `Base` did you mean" concept); disambiguating via a workspace-wide
/// short-name candidate list was solving the wrong problem; this resolves
/// it the way the language actually does. Confirmed against an indexed
/// class via `DocumentStore::class_ref_by_fqn` (O(1) FQCN lookup, not a
/// short-name scan) — an external/vendor parent with no workspace
/// `FileIndex` entry returns `None` so the caller falls back to the
/// enclosing-class heuristic, same as when nothing resolves at all.
fn resolve_parent_construct_class(
    doc: &crate::document::ast::ParsedDoc,
    position: Position,
    wi: &crate::db::workspace_index::WorkspaceIndexData,
    docs: &crate::document::document_store::DocumentStore,
    file_imports: &std::collections::HashMap<String, String>,
) -> Option<String> {
    let child_short = enclosing_class_at(doc.source(), doc, position)?;
    let raw_parent = crate::types::type_map::parent_class_name(doc, &child_short)?;
    let resolved = crate::navigation::moniker::resolve_fqn(doc, &raw_parent, file_imports);
    let resolved = resolved.trim_start_matches('\\').to_string();
    docs.class_ref_by_fqn(wi, &resolved)?;
    Some(resolved)
}

/// Byte offset of the last char of `receiver_var` in the nearest
/// `receiver_var->` / `receiver_var?->` / `receiver_var::` occurrence before
/// the cursor — a position inside mir's end-exclusive variable span. Mirrors
/// `editing/signature_help.rs::receiver_var_offset`.
fn receiver_var_offset(
    doc: &crate::document::ast::ParsedDoc,
    line_text: &str,
    position: Position,
    receiver_var: &str,
) -> Option<u32> {
    let cursor_byte = crate::text::utf16_offset_to_byte(line_text, position.character as usize)
        .min(line_text.len());
    let before = &line_text[..cursor_byte];
    let p = before
        .rfind(&format!("{receiver_var}?->"))
        .or_else(|| before.rfind(&format!("{receiver_var}->")))
        .or_else(|| before.rfind(&format!("{receiver_var}::")))?;
    let line_start = doc.view().byte_of_position(Position {
        line: position.line,
        character: 0,
    });
    Some(line_start + (p + receiver_var.len()) as u32 - 1)
}
