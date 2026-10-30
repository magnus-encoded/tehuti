//! Ranked candidates supplied by a caller, independent of their eventual action.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub name: String,
    pub meta: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Create {
    pub heading: String,
    pub detail: String,
    pub action: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Slots {
    pub title: String,
    pub heard: String,
    pub scopes: Vec<String>,
    pub create: Option<Create>,
    pub candidates: Vec<Candidate>,
    pub action: String,
    pub closest_heading: String,
    pub remaining_heading: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Create,
    /// Index in the caller's original candidate vector.
    Candidate(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ranked {
    pub candidate: Candidate,
    pub choice: Choice,
    pub close: bool,
    split: usize,
}

impl Ranked {
    /// Bright prefix and dim tail. Distant candidates have no dim tail.
    #[must_use]
    pub fn name_spans(&self) -> (&str, &str) {
        self.candidate
            .name
            .get(..self.split)
            .zip(self.candidate.name.get(self.split..))
            .unwrap_or((&self.candidate.name, ""))
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Outcome {
    #[default]
    Pending,
    Cancelled,
    Picked(Choice),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct List {
    slots: Slots,
    footer: String,
    fill: u16,
    candidates: Vec<Ranked>,
    selected: Option<usize>,
    first: usize,
    outcome: Outcome,
}

impl List {
    #[must_use]
    pub fn new(slots: Slots) -> Self {
        let heard = slots.heard.to_lowercase();
        let mut ranked: Vec<_> = slots
            .candidates
            .iter()
            .enumerate()
            .map(|(index, candidate)| {
                let name = candidate.name.to_lowercase();
                let distance = distance(&heard, &name);
                let close = !heard.is_empty() && distance <= 3;
                let split = if close {
                    prefix_end(&candidate.name, &heard)
                } else {
                    candidate.name.len()
                };
                (
                    distance,
                    name,
                    Ranked {
                        candidate: candidate.clone(),
                        choice: Choice::Candidate(index),
                        close,
                        split,
                    },
                )
            })
            .collect();
        ranked.sort_by(|left, right| {
            right
                .2
                .close
                .cmp(&left.2.close)
                .then_with(|| {
                    if left.2.close {
                        left.0.cmp(&right.0)
                    } else {
                        core::cmp::Ordering::Equal
                    }
                })
                .then_with(|| left.1.cmp(&right.1))
        });
        let candidates: Vec<_> = ranked.into_iter().map(|(_, _, row)| row).collect();
        let has_close = candidates.first().is_some_and(|row| row.close);
        let selected = if has_close {
            Some(usize::from(slots.create.is_some()))
        } else if slots.create.is_some() || !candidates.is_empty() {
            Some(0)
        } else {
            None
        };
        Self {
            slots,
            footer: String::new(),
            fill: 0,
            candidates,
            selected,
            first: 0,
            outcome: Outcome::Pending,
        }
    }

    #[must_use]
    pub fn ordered(slots: Slots) -> Self {
        let candidates = slots
            .candidates
            .iter()
            .enumerate()
            .map(|(index, candidate)| Ranked {
                candidate: candidate.clone(),
                choice: Choice::Candidate(index),
                close: false,
                split: candidate.name.len(),
            })
            .collect();
        let selected = (!slots.candidates.is_empty()).then_some(0);
        Self {
            slots,
            candidates,
            selected,
            first: 0,
            outcome: Outcome::Pending,
            footer: String::new(),
            fill: 0,
        }
    }

    pub fn set_scope(&mut self, title: String, scopes: Vec<String>) {
        self.slots.title = title;
        self.slots.scopes = scopes;
    }

    pub fn decorate(&mut self, footer: String, fill: u16) {
        self.footer = footer;
        self.fill = fill.min(1000);
    }

    #[must_use]
    pub fn footer(&self) -> &str {
        &self.footer
    }

    #[must_use]
    pub const fn fill(&self) -> u16 {
        self.fill
    }

    #[must_use]
    pub const fn slots(&self) -> &Slots {
        &self.slots
    }

    #[must_use]
    pub fn candidates(&self) -> &[Ranked] {
        &self.candidates
    }

    #[must_use]
    pub fn show_headings(&self) -> bool {
        self.candidates.first().is_some_and(|row| row.close)
            && self.candidates.last().is_some_and(|row| !row.close)
    }

    #[must_use]
    pub fn selected(&self) -> Option<Choice> {
        let index = self.selected?;
        if self.slots.create.is_some() && index == 0 {
            return Some(Choice::Create);
        }
        self.candidates
            .get(index.saturating_sub(usize::from(self.slots.create.is_some())))
            .map(|row| row.choice)
    }

    #[must_use]
    pub const fn outcome(&self) -> Outcome {
        self.outcome
    }

    #[must_use]
    pub fn is_open(&self) -> bool {
        self.outcome == Outcome::Pending
    }

    /// The candidate window; the optional create card stays above it.
    #[must_use]
    pub fn visible(&self) -> core::ops::Range<usize> {
        self.first..self.first.saturating_add(5).min(self.candidates.len())
    }

    pub fn move_selection(&mut self, down: bool) {
        if !self.is_open() {
            return;
        }
        let Some(selected) = self.selected else {
            return;
        };
        let count = self
            .candidates
            .len()
            .saturating_add(usize::from(self.slots.create.is_some()));
        let next = if down {
            selected.saturating_add(1).min(count.saturating_sub(1))
        } else {
            selected.saturating_sub(1)
        };
        self.selected = Some(next);
        let candidate = next.saturating_sub(usize::from(self.slots.create.is_some()));
        if candidate < self.first {
            self.first = candidate;
        }
        if candidate >= self.first.saturating_add(5) {
            self.first = candidate.saturating_sub(4);
        }
    }

    #[must_use]
    pub fn selected_hint(&self, family: crate::pad::PadFamily) -> Option<(crate::ticker::Piece, &str)> {
        let action = match self.selected()? {
            Choice::Create => {
                let create = self.slots.create.as_ref()?;
                if !create.enabled {
                    return None;
                }
                &create.action
            }
            Choice::Candidate(_) => &self.slots.action,
        };
        self.is_open()
            .then(|| (family.glyph(crate::pad::Role::Confirm), action.as_str()))
    }

    #[must_use]
    pub fn back_hint(&self, family: crate::pad::PadFamily) -> (crate::ticker::Piece, &'static str) {
        (family.glyph(crate::pad::Role::Back), "Close")
    }

    pub fn confirm(&mut self) {
        if self.selected_hint(crate::pad::PadFamily::Letters).is_some()
            && let Some(choice) = self.selected()
        {
            self.outcome = Outcome::Picked(choice);
        }
    }

    pub fn cancel(&mut self) {
        if self.is_open() {
            self.outcome = Outcome::Cancelled;
        }
    }
}

fn prefix_end(name: &str, heard: &str) -> usize {
    let mut remaining = heard.chars();
    let mut end = 0;
    for (index, character) in name.char_indices() {
        if character
            .to_lowercase()
            .all(|lower| Some(lower) == remaining.next())
        {
            end = index.saturating_add(character.len_utf8());
        } else {
            break;
        }
    }
    end
}

fn distance(left: &str, right: &str) -> usize {
    let mut previous: Vec<usize> = (0..=right.chars().count()).collect();
    for (row, character) in left.chars().enumerate() {
        let mut current = vec![row.saturating_add(1)];
        for (column, other) in right.chars().enumerate() {
            let insertion = current.last().copied().unwrap_or_default().saturating_add(1);
            let deletion = previous
                .get(column.saturating_add(1))
                .copied()
                .unwrap_or_default()
                .saturating_add(1);
            let substitution = previous
                .get(column)
                .copied()
                .unwrap_or_default()
                .saturating_add(usize::from(character != other));
            current.push(insertion.min(deletion).min(substitution));
        }
        previous = current;
    }
    previous.last().copied().unwrap_or_default()
}
