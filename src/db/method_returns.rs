//! `method_returns` salsa query — derives the per-file map of
//! `class_name -> method_name -> return_class_name`. Depends on `parsed_doc`.
//!
//! The old `OnceLock<MethodReturnsMap>` cache on `ParsedDoc` is left in place
//! for now; callers that go through salsa get the memoization from this
//! query instead. Phase B4 (DocumentStore wrapping) will route the remaining
//! callers through salsa and remove the `OnceLock`.

use std::sync::Arc;

use salsa::{Database, Update};

use crate::ast::MethodReturnsMap;
use crate::db::input::SourceFile;
use crate::db::parse::parsed_doc;
use crate::type_map::build_method_returns;

#[derive(Clone)]
pub struct MethodReturnsArc(pub Arc<MethodReturnsMap>);

impl MethodReturnsArc {
    #[allow(dead_code)] // Used by tests and by Phase E call sites.
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
