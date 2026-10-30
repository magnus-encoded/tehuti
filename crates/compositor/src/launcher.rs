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
        if kind == tehuti_core::unit_verbs::DestinationKind::Creation {
            continue;
        }
        if kind == tehuti_core::unit_verbs::DestinationKind::Project && entry.source != Source::Project {
            continue;
        }
        for route in &entry.routes {
            let open = route
                .steps
                .iter()
                .any(|step| matches!(step.target, Target::Window(_)));
            let description = route
                .steps
                .iter()
                .map(|step| match &step.target {
                    Target::Desktop(value)
                    | Target::Url(value)
                    | Target::Application(value)
                    | Target::Menu(value) => {
                        format!("{}: {value}", step.icon)
                    }
                    Target::Workspace(_) => format!("{}: workspace", step.icon),
                    Target::Window(_) => format!("{}: open window", step.icon),
                    Target::Action(_) => format!("{}: {}", step.icon, entry.name),
                })
                .collect::<Vec<_>>()
                .join(" → ");
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
