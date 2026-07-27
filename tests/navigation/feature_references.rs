//! Comprehensive reference/find-usages (textDocument/references) coverage via the annotation DSL.
//!
//! Tests are written so the fixture itself specifies where references should
//! land — `// ^^^ def` for the declaration and `// ^^^ ref` for each use
//! site. `check_references_annotated` fails with a side-by-side diff if the
//! server returns anything missing or extra.

use super::*;

#[path = "references/basic.rs"]
mod basic;

#[path = "references/cross_file.rs"]
mod cross_file;

#[path = "references/oop.rs"]
mod oop;

#[path = "references/visibility.rs"]
mod visibility;

#[path = "references/stress.rs"]
mod stress;

#[path = "references/constructors.rs"]
mod constructors;

#[path = "references/properties.rs"]
mod properties;

#[path = "references/protocol.rs"]
mod protocol;

#[path = "references/workspace_scan.rs"]
mod workspace_scan;

#[path = "references/cold_candidate_gating.rs"]
mod cold_candidate_gating;

#[path = "references/expressions.rs"]
mod expressions;

#[path = "references/edge_cases.rs"]
mod edge_cases;

#[path = "references/constants.rs"]
mod constants;

#[path = "references/static_properties.rs"]
mod static_properties;

#[path = "references/advanced_types.rs"]
mod advanced_types;

#[path = "references/attributes_complete.rs"]
mod attributes_complete;

#[path = "references/variables.rs"]
mod variables;

#[path = "references/partial_result.rs"]
mod partial_result;

#[path = "references/fqn_narrowing.rs"]
mod fqn_narrowing;
