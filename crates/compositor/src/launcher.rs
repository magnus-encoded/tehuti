//! Converts discovered routes into launcher rows.
use crate::destinations::{Destination, Route};
use tehuti_core::unit_verbs::DestinationKind;

#[must_use]
pub fn destinations(
    entries: &[Destination],
    kind: DestinationKind,
    focused: Option<&str>,
) -> (Vec<tehuti_core::launcher::Destination>, Vec<Route>) {
    use crate::destinations::{Source, Target};
    let mut destinations = Vec::new();
    let mut routes = Vec::new();
    for entry in entries {
        if kind == DestinationKind::Creation
            || (kind == DestinationKind::Project && entry.source != Source::Project)
        {
            continue;
        }
        for (index, route) in entry.routes.iter().enumerate() {
            if entry.source == Source::Desktop && index > 0 {
                continue;
            }
            if duplicates_desktop_route(route, entries) {
                continue;
            }
            let open = route
                .steps
                .iter()
                .any(|step| matches!(step.target, Target::Window(_)));
            let description = describe(route, &entry.name);
            destinations.push(tehuti_core::launcher::Destination {
                name: entry.name.clone(),
                extra_names: entry.extra_names.clone(),
                route: description,
                open,
                focused: route
                    .context
                    .as_deref()
                    .is_some_and(|context| Some(context) == focused),
                safety: if route
                    .steps
                    .iter()
                    .any(|step| matches!(step.target, Target::Action(_) | Target::Menu(_)))
                {
                    tehuti_core::launcher::Safety::Changes
                } else if open {
                    tehuti_core::launcher::Safety::Focus
                } else {
                    tehuti_core::launcher::Safety::Launch
                },
            });
            routes.push(route.clone());
        }
    }
    (destinations, routes)
}

fn describe(route: &Route, name: &str) -> String {
    use crate::destinations::Target;
    route
        .steps
        .iter()
        .map(|step| match &step.target {
            Target::Desktop(value)
            | Target::Url(value)
            | Target::Application(value)
            | Target::Menu(value) => format!("{}: {value}", step.icon),
            Target::Workspace(_) => format!("{}: workspace", step.icon),
            Target::Window(_) => format!("{}: open window", step.icon),
            Target::Action(_) => format!("{}: {name}", step.icon),
        })
        .collect::<Vec<_>>()
        .join(" → ")
}

fn duplicates_desktop_route(route: &Route, entries: &[Destination]) -> bool {
    use crate::destinations::{Source, Target};
    let [step] = route.steps.as_slice() else {
        return false;
    };
    let Target::Application(application) = &step.target else {
        return false;
    };
    entries.iter().any(|entry| {
        entry.source == Source::Desktop
            && entry.routes.iter().any(|desktop_route| {
                desktop_route.steps.iter().any(|desktop_step| {
                    matches!(&desktop_step.target,
                        Target::Desktop(desktop_id)
                            if desktop_id.strip_suffix(".desktop") == Some(application.as_str()))
                })
            })
    })
}
