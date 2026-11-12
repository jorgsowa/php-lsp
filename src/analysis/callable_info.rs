use crate::lang::doc_html::html_to_markdown;

pub(crate) struct CallableInfo {
    pub params: String,
    pub param_labels: Vec<ParamLabel>,
    #[cfg_attr(target_family = "wasm", allow(dead_code))]
    pub return_type: Option<String>,
    pub documentation: Option<String>,
}

/// One rendered parameter of a callable signature.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ParamLabel {
    /// Parameter name without `$`.
    pub name: String,
    pub label: String,
}

impl ParamLabel {
    pub(crate) fn join(params: &[ParamLabel]) -> String {
        params
            .iter()
            .map(|p| p.label.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

pub(crate) fn declared_param_labels(params: &[mir_analyzer::DeclaredParam]) -> Vec<ParamLabel> {
    params
        .iter()
        .map(|p| {
            let name = p.name.as_str().trim_start_matches('$');
            let mut s = String::new();
            if let Some(ty) = &p.ty {
                s.push_str(&format!("{ty} "));
            }
            if p.is_byref {
                s.push('&');
            }
            if p.is_variadic {
                s.push_str("...");
            }
            s.push_str(&format!("${name}"));
            if let Some(text) = &p.default_text {
                s.push_str(&format!(" = {text}"));
            } else if p.has_default {
                s.push_str(" = ...");
            }
            ParamLabel {
                name: name.to_string(),
                label: s,
            }
        })
        .collect()
}

#[cfg_attr(target_family = "wasm", allow(dead_code))]
pub(crate) fn format_declared_params(params: &[mir_analyzer::DeclaredParam]) -> String {
    ParamLabel::join(&declared_param_labels(params))
}

fn callable_info(
    params: &[mir_analyzer::DeclaredParam],
    return_type: Option<String>,
    documentation: Option<String>,
) -> CallableInfo {
    let param_labels = declared_param_labels(params);
    CallableInfo {
        params: ParamLabel::join(&param_labels),
        param_labels,
        return_type,
        documentation,
    }
}

pub(crate) fn callable_info_for_name(
    db: &mir_analyzer::db::MirDbStorage,
    symbol: &mir_analyzer::Name,
) -> Option<CallableInfo> {
    match symbol {
        mir_analyzer::Name::Function(fqn) => {
            let f = mir_analyzer::db::find_function(
                db,
                mir_analyzer::db::Fqcn::from_str(db, fqn.as_ref()),
            )?;
            Some(callable_info(
                &f.params,
                f.effective_return_type().map(ToString::to_string),
                f.docstring.as_deref().map(html_to_markdown),
            ))
        }
        mir_analyzer::Name::Method { class, name } => {
            let (_, m) = mir_analyzer::db::find_method_in_chain(
                db,
                mir_analyzer::db::Fqcn::from_str(db, class.as_ref()),
                name.as_ref(),
            )?;
            Some(callable_info(
                &m.params,
                m.return_type
                    .as_deref()
                    .or(m.inferred_return_type.as_deref())
                    .map(ToString::to_string),
                m.docstring.as_deref().map(html_to_markdown),
            ))
        }
        mir_analyzer::Name::Class(fqcn) => {
            let (_, m) = mir_analyzer::db::find_method_in_chain(
                db,
                mir_analyzer::db::Fqcn::from_str(db, fqcn.as_ref()),
                "__construct",
            )?;
            Some(callable_info(
                &m.params,
                None,
                m.docstring.as_deref().map(html_to_markdown),
            ))
        }
        _ => None,
    }
}

/// Whether the parameter of `symbol` that takes the `position`-th (or `name`d) argument is `&`.
#[cfg_attr(target_family = "wasm", allow(dead_code))]
pub(crate) fn param_is_byref(
    db: &mir_analyzer::db::MirDbStorage,
    symbol: &mir_analyzer::Name,
    position: usize,
    name: Option<&str>,
) -> bool {
    let params = match symbol {
        mir_analyzer::Name::Function(fqn) => {
            mir_analyzer::db::find_function(db, mir_analyzer::db::Fqcn::from_str(db, fqn.as_ref()))
                .map(|f| f.params.clone())
        }
        mir_analyzer::Name::Method { class, name } => mir_analyzer::db::find_method_in_chain(
            db,
            mir_analyzer::db::Fqcn::from_str(db, class.as_ref()),
            name.as_ref(),
        )
        .map(|(_, m)| m.params.clone()),
        mir_analyzer::Name::Class(fqcn) => mir_analyzer::db::find_method_in_chain(
            db,
            mir_analyzer::db::Fqcn::from_str(db, fqcn.as_ref()),
            "__construct",
        )
        .map(|(_, m)| m.params.clone()),
        _ => None,
    };
    let Some(params) = params else {
        return false;
    };
    let param = match name {
        Some(name) => params
            .iter()
            .find(|p| p.name.as_str().trim_start_matches('$') == name),
        None => params
            .get(position)
            .or_else(|| params.last().filter(|p| p.is_variadic)),
    };
    param.is_some_and(|p| p.is_byref)
}
