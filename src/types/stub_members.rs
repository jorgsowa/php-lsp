use mir_analyzer::AnalysisSnapshot;
use mir_analyzer::db::{Fqcn, find_class_like};

use crate::types::type_map::ClassMembers;

/// Look up class members for a built-in PHP class by querying phpstorm-stubs
/// through a snapshot. Returns `None` when `fqcn` is not a known built-in
/// class or a workspace class shadows it. The snapshot loads the stub file
/// defining `fqcn` on demand as a pure read, so it works even when no
/// analyzed file references the class.
pub fn stub_class_members(
    snapshot: &AnalysisSnapshot,
    fqcn: &str,
) -> Result<Option<ClassMembers>, salsa::Cancelled> {
    let normalized = fqcn.strip_prefix('\\').unwrap_or(fqcn);
    if !snapshot.is_builtin_class(normalized)? {
        return Ok(None);
    }
    snapshot.read(|db| {
        let key = Fqcn::from_str(db, normalized);
        let class_like = find_class_like(db, key)?;
        let mut members = ClassMembers {
            found: true,
            ..Default::default()
        };
        // Map keys are lowercased; `method.name` keeps the declared case.
        for method in class_like.own_methods().values() {
            members.methods.push((
                method.name.to_string(),
                method.is_static,
                !method.params.is_empty(),
            ));
        }
        if let Some(props) = class_like.own_properties() {
            for (name, prop) in props {
                members.properties.push((name.to_string(), prop.is_static));
            }
        }
        for name in class_like.own_constants().keys() {
            members.constants.push(name.to_string());
        }
        members.parent = class_like.parent().map(|p| p.to_string());
        Some(members)
    })
}
