//! `method_returns` salsa query — derives the per-file map of
//! `class_name -> method_name -> return_class_name`. Depends on `parsed_doc`.
//!
//! This is the sole cache for method-return inference since Phase E3 removed
//! the `OnceLock<MethodReturnsMap>` from `ParsedDoc`. Production call sites
//! (inlay_hints, type_definition, hover, completion) fetch the memoized
//! `Arc<MethodReturnsMap>` via `DocumentStore::get_method_returns_salsa` /
//! `other_docs_with_returns` and pass it into the `TypeMap` constructors.

use std::sync::Arc;

use salsa::{Database, Update};

use crate::ast::MethodReturnsMap;
use crate::db::input::SourceFile;
use crate::db::parse::parsed_doc;
use crate::type_map::build_method_returns;

#[derive(Clone)]
pub struct MethodReturnsArc(pub Arc<MethodReturnsMap>);

impl MethodReturnsArc {
    pub fn get(&self) -> &MethodReturnsMap {
        &self.0
    }
}

// SAFETY: identical contract to `ParsedArc::maybe_update`.
unsafe impl Update for MethodReturnsArc {
    unsafe fn maybe_update(old_pointer: *mut Self, new_value: Self) -> bool {
        let old_ref = unsafe { &mut *old_pointer };
        if Arc::ptr_eq(&old_ref.0, &new_value.0) {
            false
        } else {
            *old_ref = new_value;
            true
        }
    }
}

#[salsa::tracked(no_eq)]
pub fn method_returns(db: &dyn Database, file: SourceFile) -> MethodReturnsArc {
    let doc = parsed_doc(db, file);
    MethodReturnsArc(Arc::new(build_method_returns(doc.get())))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::db::analysis::AnalysisHost;
    use crate::db::input::{FileId, SourceFile};
    use salsa::Setter;

    #[test]
    fn method_returns_captures_factory_return() {
        let host = AnalysisHost::new();
        let file = SourceFile::new(
            host.db(),
            FileId(0),
            Arc::<str>::from("file:///t.php"),
            Arc::<str>::from(
                "<?php\nclass Foo {\n    public function make(): Bar { return new Bar(); }\n}\nclass Bar {}",
            ),
            None,
        );
        let m = method_returns(host.db(), file);
        let foo = m.get().get("Foo").expect("class Foo in map");
        assert_eq!(foo.get("make").map(String::as_str), Some("Bar"));
    }

    #[test]
    fn method_returns_reruns_after_edit() {
        let mut host = AnalysisHost::new();
        let file = SourceFile::new(
            host.db(),
            FileId(1),
            Arc::<str>::from("file:///t.php"),
            Arc::<str>::from(
                "<?php\nclass F {\n    public function m(): A { return new A(); }\n}\nclass A {}\nclass B {}",
            ),
            None,
        );
        let a1 = method_returns(host.db(), file);
        let first = Arc::as_ptr(&a1.0);

        file.set_text(host.db_mut()).to(Arc::<str>::from(
            "<?php\nclass F {\n    public function m(): B { return new B(); }\n}\nclass A {}\nclass B {}",
        ));
        let a2 = method_returns(host.db(), file);
        assert_ne!(
            first,
            Arc::as_ptr(&a2.0),
            "editing the source should produce a new map"
        );
        assert_eq!(
            a2.get()
                .get("F")
                .and_then(|m| m.get("m"))
                .map(String::as_str),
            Some("B")
        );
    }
}
