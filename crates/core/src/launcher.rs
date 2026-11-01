//! Pure launcher ranking, controller selection and conservative automatic picking.
use crate::choice_list::{Candidate, Choice, List, Slots};
use core::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Safety {
    Focus,
    Launch,
    Prompts,
    Changes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Destination {
    pub name: String,
    pub extra_names: Vec<String>,
    pub route: String,
    pub open: bool,
    pub focused: bool,
    pub safety: Safety,
}

impl Destination {
    fn names(&self) -> impl Iterator<Item = &str> {
        core::iter::once(self.name.as_str()).chain(self.extra_names.iter().map(String::as_str))
    }

    fn similarity(&self, folded: &str) -> u16 {
        self.names()
            .map(|name| tehuti_protocol::similarity(folded, &tehuti_protocol::fold_phrase(name)))
            .max()
            .unwrap_or_default()
    }

    fn exactly_matches(&self, normalized: &str) -> bool {
        self.names()
            .any(|name| normalized == tehuti_protocol::normalize_phrase(name))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Checks {
    pub heard: bool,
    pub matched: bool,
    pub safe: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launcher {
    list: List,
    destinations: Vec<Destination>,
    indices: Vec<usize>,
    checks: Checks,
    elapsed: Option<Duration>,
    jev_elapsed: Option<Duration>,
    interrupted: bool,
    enabled: bool,
    delay: Duration,
    invitation: bool,
    open_for: Duration,
    usage: Option<Box<Usage>>,
}

/// How one launcher use ended. Ranks count from 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    Chosen {
        rank: usize,
    },
    Cancelled,
    /// Speech arrived while open and a new launcher took its place.
    Replaced,
}

/// One finished launcher use: the heard query, the ranked candidate names and how it ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Usage {
    pub query: String,
    pub candidates: Vec<String>,
    pub ending: Ending,
    /// Time from the transcript opening the launcher to its ending.
    pub since_transcript: Duration,
}

/// Counts over logged launcher uses.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub uses: usize,
    pub chosen: usize,
    pub first_rank: usize,
    pub cancelled: usize,
    pub replaced: usize,
    /// Heard query and the chosen name, where the two differ after normalizing.
    pub mishears: Vec<(String, String)>,
}

impl Report {
    #[must_use]
    pub fn from_uses(uses: &[Usage]) -> Self {
        let mut report = Self {
            uses: uses.len(),
            ..Self::default()
        };
        for usage in uses {
            match usage.ending {
                Ending::Chosen { rank } => {
                    report.chosen = report.chosen.saturating_add(1);
                    if rank == 1 {
                        report.first_rank = report.first_rank.saturating_add(1);
                    }
                    if let Some(name) = rank.checked_sub(1).and_then(|index| usage.candidates.get(index))
                        && tehuti_protocol::normalize_phrase(name)
                            != tehuti_protocol::normalize_phrase(&usage.query)
                    {
                        report.mishears.push((usage.query.clone(), name.clone()));
                    }
                }
                Ending::Cancelled => report.cancelled = report.cancelled.saturating_add(1),
                Ending::Replaced => report.replaced = report.replaced.saturating_add(1),
            }
        }
        report
    }
}

impl Launcher {
    #[must_use]
    pub fn new(query: &str, destinations: Vec<Destination>, heard: Option<u16>) -> Self {
        let mut rows = local_ranking(query, &destinations);
        rows.truncate(5);
        let matched = rows.first().is_some_and(|first| {
            destinations.get(first.0).is_some_and(|destination| {
                destination.similarity(&tehuti_protocol::fold_phrase(query)) >= 850
            }) && rows
                .get(1)
                .is_none_or(|second| first.1.saturating_sub(second.1) >= 100)
        });
        let safe = rows
            .first()
            .and_then(|(index, _)| destinations.get(*index))
            .is_some_and(|destination| destination.safety != Safety::Changes);
        let checks = Checks {
            heard: heard.is_some_and(|value| value >= 900),
            matched,
            safe,
        };
        let elapsed = (checks.heard && checks.matched && checks.safe).then_some(Duration::ZERO);
        let indices: Vec<_> = rows.into_iter().map(|(index, _)| index).collect();
        let list = List::ordered(Slots {
            title: "Open".into(),
            heard: query.into(),
            action: "Open".into(),
            candidates: indices
                .iter()
                .filter_map(|index| destinations.get(*index))
                .map(|destination| Candidate {
                    name: destination.name.clone(),
                    meta: Some(destination.route.clone()),
                })
                .collect(),
            ..Slots::default()
        });
        let mut launcher = Self {
            list,
            destinations,
            indices,
            checks,
            elapsed,
            jev_elapsed: None,
            interrupted: false,
            enabled: true,
            delay: Duration::from_secs(5),
            invitation: false,
            open_for: Duration::ZERO,
            usage: None,
        };
        launcher.decorate();
        launcher
    }

    /// The session-start prompt: nothing is open yet, so speech without a verb opens.
    /// `button` names this controller's push-to-talk button; `None` means no controller.
    #[must_use]
    pub fn invitation(button: Option<&str>) -> Self {
        let (title, scopes) = button.map_or_else(
            || {
                (
                    "No controller connected".to_owned(),
                    vec![
                        "Connect a controller to speak,".to_owned(),
                        "or type what to open and press Enter.".to_owned(),
                    ],
                )
            },
            |button| {
                (
                    format!("Hold {button} and speak"),
                    vec![
                        "Say what to open, for example:".to_owned(),
                        "“firefox”  ·  “open terminal”  ·  “work on notes”".to_owned(),
                    ],
                )
            },
        );
        let mut slots = Slots {
            title,
            action: "Open".into(),
            ..Slots::default()
        };
        slots.scopes = scopes;
        let mut launcher = Self::new("", Vec::new(), None);
        launcher.list = List::ordered(slots);
        launcher.invitation = true;
        launcher.decorate();
        launcher
    }

    #[must_use]
    pub const fn is_invitation(&self) -> bool {
        self.invitation
    }

    pub fn set_scope(&mut self, verb: &str, level: &str, focused_app: Option<&str>) {
        let title = if verb == "New" && self.list.slots().heard.is_empty() {
            "New what?".into()
        } else {
            format!("{verb} {}", self.list.slots().heard)
        };
        let scope = focused_app.map_or_else(|| level.to_owned(), |app| format!("{level} · {app}"));
        self.list.set_scope(title, vec![scope]);
    }

    #[must_use]
    pub const fn list(&self) -> &List {
        &self.list
    }

    #[must_use]
    pub fn jev_request(
        &mut self,
        enabled: bool,
        key_present: bool,
        level: &str,
        focused_app: Option<&str>,
    ) -> Option<JevRequest> {
        if !enabled || !key_present || self.indices.is_empty() {
            return None;
        }
        self.jev_elapsed = Some(Duration::ZERO);
        Some(JevRequest {
            query: self.list.slots().heard.clone(),
            level: level.into(),
            focused_app: focused_app.map(str::to_owned),
            candidates: self
                .list
                .candidates()
                .iter()
                .map(|row| row.candidate.clone())
                .collect(),
        })
    }

    pub fn jev_reply(&mut self, confidences: &[u16]) {
        if !self.list.is_open()
            || self.interrupted
            || self
                .jev_elapsed
                .take()
                .is_none_or(|elapsed| elapsed > Duration::from_millis(1500))
            || confidences.len() != self.indices.len()
            || confidences.iter().any(|score| *score > 1000)
        {
            return;
        }
        let mut ranked: Vec<_> = self
            .indices
            .iter()
            .copied()
            .zip(confidences.iter().copied())
            .collect();
        ranked.sort_by_key(|row| core::cmp::Reverse(row.1));
        self.checks.matched = ranked.first().is_some_and(|first| {
            first.1 >= 850
                && ranked
                    .get(1)
                    .is_none_or(|second| first.1.saturating_sub(second.1) >= 100)
        });
        self.indices = ranked.into_iter().map(|(index, _)| index).collect();
        self.checks.safe = self
            .indices
            .first()
            .and_then(|index| self.destinations.get(*index))
            .is_some_and(|row| row.safety != Safety::Changes);
        let mut slots = self.list.slots().clone();
        slots.candidates = self
            .indices
            .iter()
            .filter_map(|index| self.destinations.get(*index))
            .map(|row| Candidate {
                name: row.name.clone(),
                meta: Some(row.route.clone()),
            })
            .collect();
        self.list = List::ordered(slots);
        self.elapsed = if self.enabled && self.checks.heard && self.checks.matched && self.checks.safe {
            Some(Duration::ZERO)
        } else {
            None
        };
        self.decorate();
    }

    fn decorate(&mut self) {
        let fill = self
            .elapsed
            .map(|elapsed| {
                u16::try_from(
                    elapsed
                        .as_millis()
                        .saturating_mul(1000)
                        .checked_div(self.delay.as_millis())
                        .unwrap_or_default()
                        .min(1000),
                )
                .unwrap_or_default()
            })
            .unwrap_or_default();
        let footer = self.footer().to_owned();
        self.list.decorate(footer, fill);
    }

    pub fn confirm(&mut self) -> Option<usize> {
        if !self.list.is_open() {
            return None;
        }
        self.list.confirm();
        match self.list.selected()? {
            Choice::Candidate(index) => {
                if !self.list.is_open() {
                    self.finish(Ending::Chosen {
                        rank: index.saturating_add(1),
                    });
                }
                self.indices.get(index).copied()
            }
            Choice::Create => None,
        }
    }

    fn finish(&mut self, ending: Ending) {
        if self.invitation || self.usage.is_some() {
            return;
        }
        self.usage = Some(Box::new(Usage {
            query: self.list.slots().heard.clone(),
            candidates: self
                .list
                .candidates()
                .iter()
                .map(|row| row.candidate.name.clone())
                .collect(),
            ending,
            since_transcript: self.open_for,
        }));
    }

    /// The finished use, once, after a confirm, cancel or replacement. The invitation has none.
    pub fn take_usage(&mut self) -> Option<Usage> {
        self.usage.take().map(|usage| *usage)
    }

    /// Ends this use because new speech opens another launcher.
    pub fn replace(&mut self) {
        if self.list.is_open() {
            self.list.cancel();
            self.finish(Ending::Replaced);
        }
    }

    #[must_use]
    pub const fn checks(&self) -> Checks {
        self.checks
    }

    #[must_use]
    pub const fn footer(&self) -> &str {
        if self.invitation {
            "Nothing is open yet"
        } else if self.indices.is_empty() {
            "Nothing found"
        } else if !self.enabled {
            "Auto-pick is off"
        } else if !self.checks.heard {
            "Not sure what I heard"
        } else if !self.checks.matched {
            "Close call"
        } else if !self.checks.safe {
            "Requires confirmation"
        } else if self.elapsed.is_none() {
            "Confirm to open"
        } else {
            "Opening automatically"
        }
    }

    pub fn configure(&mut self, enabled: bool, delay: Duration) {
        self.enabled = enabled;
        self.delay = delay.max(Duration::from_millis(1));
        if !enabled {
            self.elapsed = None;
        }
        self.decorate();
    }

    pub fn stop_timer(&mut self) {
        self.elapsed = None;
        self.interrupted = true;
        self.decorate();
    }

    pub fn tick(&mut self, elapsed: Duration) -> Option<usize> {
        if !self.list.is_open() {
            return None;
        }
        self.open_for = self.open_for.saturating_add(elapsed);
        if let Some(age) = &mut self.jev_elapsed {
            *age = age.saturating_add(elapsed);
        }
        let current = self.elapsed.as_mut()?;
        *current = current.saturating_add(elapsed);
        if *current >= self.delay {
            self.confirm()
        } else {
            self.decorate();
            None
        }
    }

    pub fn move_selection(&mut self, down: bool) {
        self.stop_timer();
        self.list.move_selection(down);
    }
    pub fn cancel(&mut self) {
        if self.list.is_open() {
            self.list.cancel();
            self.finish(Ending::Cancelled);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JevRequest {
    pub query: String,
    pub level: String,
    pub focused_app: Option<String>,
    pub candidates: Vec<Candidate>,
}

fn local_ranking(query: &str, destinations: &[Destination]) -> Vec<(usize, u16)> {
    let normalized = tehuti_protocol::normalize_phrase(query);
    let folded = tehuti_protocol::fold_phrase(query);
    let mut rows: Vec<_> = destinations
        .iter()
        .enumerate()
        .filter_map(|(index, destination)| {
            let score = destination.similarity(&folded);
            let exact = destination.exactly_matches(&normalized);
            (exact || (folded.chars().count() >= tehuti_protocol::MIN_FUZZY_LETTERS && score >= 500))
                .then_some((
                    index,
                    score
                        .saturating_add(if destination.open { 80 } else { 0 })
                        .saturating_add(if destination.focused { 80 } else { 0 }),
                ))
        })
        .collect();
    rows.sort_by_key(|row| {
        core::cmp::Reverse((
            destinations
                .get(row.0)
                .is_some_and(|destination| destination.exactly_matches(&normalized)),
            row.1,
        ))
    });
    rows
}
