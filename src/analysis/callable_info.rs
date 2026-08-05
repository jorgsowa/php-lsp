pub(crate) struct CallableInfo {
    pub params: String,
    pub return_type: Option<String>,
    pub documentation: Option<String>,
}

pub(crate) fn format_declared_params(params: &[mir_analyzer::DeclaredParam]) -> String {
    params
        .iter()
        .map(|p| {
            let mut s = String::new();
            if let Some(ty) = &p.ty {
                s.push_str(&format!("{ty} "));
            }
            if p.is_variadic {
                s.push_str("...");
            }
            s.push_str(&format!("${}", p.name.as_str().trim_start_matches('$')));
            if p.has_default {
                s.push_str(" = ...");
            }
            s
        })
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn callable_info_for_name(
    session: &mir_analyzer::AnalysisSession,
    symbol: &mir_analyzer::Name,
) -> Option<CallableInfo> {
    let db = session.snapshot_db();
    match symbol {
        mir_analyzer::Name::Function(fqn) => {
            let short = fqn.rsplit('\\').next().unwrap_or(fqn.as_ref());
            if mir_analyzer::is_builtin_function(short) {
                return None;
            }
            let f = mir_analyzer::db::find_function(
                &db,
                mir_analyzer::db::Fqcn::from_str(&db, fqn.as_ref()),
            )?;
            Some(CallableInfo {
                params: format_declared_params(&f.params),
                return_type: f.effective_return_type().map(ToString::to_string),
                documentation: f.docstring.as_deref().map(str::to_string),
            })
        }
        mir_analyzer::Name::Method { class, name } => {
            let (_, m) = mir_analyzer::db::find_method_in_chain(
                &db,
                mir_analyzer::db::Fqcn::from_str(&db, class.as_ref()),
                name.as_ref(),
            )?;
            Some(CallableInfo {
                params: format_declared_params(&m.params),
                return_type: m
                    .return_type
                    .as_deref()
                    .or(m.inferred_return_type.as_deref())
                    .map(ToString::to_string),
                documentation: m.docstring.as_deref().map(str::to_string),
            })
        }
        mir_analyzer::Name::Class(fqcn) => {
            let (_, m) = mir_analyzer::db::find_method_in_chain(
                &db,
                mir_analyzer::db::Fqcn::from_str(&db, fqcn.as_ref()),
                "__construct",
            )?;
            Some(CallableInfo {
                params: format_declared_params(&m.params),
                return_type: None,
                documentation: m.docstring.as_deref().map(str::to_string),
            })
        }
        _ => None,
    }
}
