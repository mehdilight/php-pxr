//! Flatten native PHP trait composition before class/generic analysis.
use super::*;

pub(super) fn expand(program: &ast::Program) -> (ast::Program, Diagnostics) {
    let mut diags = Diagnostics::new();
    let mut traits = HashMap::new();
    let mut names = std::collections::HashSet::new();
    for item in &program.items {
        if let ast::Stmt::ClassDecl(class) = item {
            let key = class.name.to_ascii_lowercase();
            if !names.insert(key.clone()) {
                diags.emit(
                    Diagnostic::error(format!("duplicate type or trait `{}`", class.name))
                        .with_code("E0201")
                        .with_span(class.name_span),
                );
            }
            if class.is_trait {
                traits.insert(key, (**class).clone());
            }
        }
    }
    let mut result = program.clone();
    let mut budget = 4096;
    result
        .items
        .retain(|item| !matches!(item, ast::Stmt::ClassDecl(class) if class.is_trait));
    for item in &mut result.items {
        if diags.has_errors() {
            break;
        }
        if let ast::Stmt::ClassDecl(class) = item {
            compose(class, &traits, &mut Vec::new(), &mut diags, &mut budget);
        }
    }
    // Parse/shape errors in unused traits remain errors as well.
    for source in traits.values() {
        if diags.has_errors() {
            break;
        }
        let mut source = source.clone();
        let key = source.name.to_ascii_lowercase();
        compose(
            &mut source,
            &traits,
            &mut vec![key],
            &mut diags,
            &mut budget,
        );
    }
    (result, diags)
}

fn compose(
    class: &mut ast::ClassDecl,
    traits: &HashMap<String, ast::ClassDecl>,
    stack: &mut Vec<String>,
    diags: &mut Diagnostics,
    budget: &mut usize,
) {
    if !class.is_trait && class.trait_uses.is_empty() {
        return;
    }
    if *budget == 0 || stack.len() > 64 {
        diags.emit(
            Diagnostic::error("trait composition limit exceeded")
                .with_code("E0285")
                .with_span(class.name_span),
        );
        return;
    }
    *budget -= 1;
    let mut error = |message: String, span: Span| {
        diags.emit(
            Diagnostic::error(message)
                .with_code("E0285")
                .with_span(span),
        )
    };
    if class.is_trait
        && (!class.type_params.is_empty()
            || class.extends.is_some()
            || !class.interfaces.is_empty()
            || class.is_abstract)
    {
        error("traits cannot declare type parameters, inheritance, interfaces, or an abstract modifier".into(), class.name_span);
    }
    if class.is_trait
        && class
            .members
            .iter()
            .any(|member| matches!(member, ast::ClassMember::Method(method) if method.is_abstract))
    {
        error(
            "abstract trait requirements are not supported; use an interface contract".into(),
            class.name_span,
        );
    }
    if class.is_interface && !class.trait_uses.is_empty() {
        error("interfaces cannot compose traits".into(), class.name_span);
    }
    let own_methods: std::collections::HashSet<_> = class
        .members
        .iter()
        .filter_map(|member| match member {
            ast::ClassMember::Method(method) => Some(method.name.to_ascii_lowercase()),
            _ => None,
        })
        .collect();
    let mut properties: std::collections::HashSet<_> = class
        .members
        .iter()
        .filter_map(|member| match member {
            ast::ClassMember::Property(property) => Some(property.name.clone()),
            _ => None,
        })
        .collect();
    let mut methods = std::collections::HashSet::new();
    let mut imported = Vec::new();
    for reference in std::mem::take(&mut class.trait_uses) {
        if diags.has_errors() {
            break;
        }
        let key = reference.name.to_ascii_lowercase();
        if !reference.type_args.is_empty() || stack.contains(&key) {
            diags.emit(
                Diagnostic::error("invalid generic or cyclic trait composition")
                    .with_code("E0285")
                    .with_span(reference.span),
            );
            continue;
        }
        let Some(mut source) = traits.get(&key).cloned() else {
            diags.emit(
                Diagnostic::error(format!("unknown trait `{}`", reference.name))
                    .with_code("E0285")
                    .with_span(reference.span),
            );
            continue;
        };
        stack.push(key);
        compose(&mut source, traits, stack, diags, budget);
        stack.pop();
        for member in source.members {
            match &member {
                ast::ClassMember::Method(method) => {
                    let key = method.name.to_ascii_lowercase();
                    if own_methods.contains(&key) {
                        continue;
                    }
                    if !methods.insert(key) {
                        diags.emit(
                            Diagnostic::error(format!(
                                "conflicting trait method `{}`; provide a class implementation",
                                method.name
                            ))
                            .with_code("E0285")
                            .with_span(class.name_span),
                        );
                        continue;
                    }
                }
                ast::ClassMember::Property(property) => {
                    if !properties.insert(property.name.clone()) {
                        diags.emit(
                            Diagnostic::error(format!(
                                "duplicate trait property `${}`",
                                property.name
                            ))
                            .with_code("E0285")
                            .with_span(class.name_span),
                        );
                        continue;
                    }
                }
            }
            imported.push(member);
        }
    }
    class.members.extend(imported);
}
