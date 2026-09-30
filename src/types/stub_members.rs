use mir_analyzer::AnalysisSession;
use mir_analyzer::db::{Fqcn, find_class_like};

use crate::types::type_map::ClassMembers;

/// Look up class members for a built-in PHP class by querying phpstorm-stubs
/// through the mir-analyzer session. Returns `None` when `fqcn` is not a
/// known built-in class. Stubs load lazily: this faults in the single stub
/// file defining `fqcn` (no-op once loaded) before reading, so it works
/// even when the class is not referenced by any analyzed file.
pub fn stub_class_members(session: &mut AnalysisSession, fqcn: &str) -> Option<ClassMembers> {
    let normalized = fqcn.strip_prefix('\\').unwrap_or(fqcn);
    if !session.ensure_stub_for_class(normalized) {
        return None;
    }
    session.read(|db| {
        let key = Fqcn::from_str(db, normalized);
        let class_like = find_class_like(db, key)?;
        let mut members = ClassMembers {
            found: true,
            ..Default::default()
        };
        for (name, method) in class_like.own_methods() {
            members.methods.push((
                name.to_string(),
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
