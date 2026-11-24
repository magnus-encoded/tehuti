//! Ranking of secret entry names for a spoken title. Pure: it sees names and
//! context, never values.

/// What the user is doing when asking, used to rank near matches.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchContext {
    /// Name of the focused application, if known.
    pub app: Option<String>,
    /// Name of the focused field, if known.
    pub field: Option<String>,
}

const EXACT: u32 = 400;
const PREFIX: u32 = 300;
const ALL_WORDS: u32 = 200;
const NAME_SPOKEN: u32 = 150;
const SUBSTRING: u32 = 100;
const CONTEXT_BONUS: u32 = 10;

fn words(text: &str) -> Vec<String> {
    text.split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn title_score(label: &str, query: &str) -> Option<u32> {
    let label = label.to_lowercase();
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Some(0);
    }
    if label == query {
        return Some(EXACT);
    }
    if label.starts_with(&query) {
        return Some(PREFIX);
    }
    let label_words = words(&label);
    let query_words = words(&query);
    if !query_words.is_empty()
        && query_words
            .iter()
            .all(|word| label_words.iter().any(|candidate| candidate == word))
    {
        return Some(ALL_WORDS);
    }
    if !label_words.is_empty() && label_words.iter().all(|word| query_words.contains(word)) {
        return Some(NAME_SPOKEN);
    }
    label.contains(&query).then_some(SUBSTRING)
}

fn context_score(label: &str, context: &SearchContext) -> u32 {
    let label_words = words(label);
    [&context.app, &context.field]
        .into_iter()
        .flatten()
        .flat_map(|text| words(text))
        .filter(|word| label_words.contains(word))
        .fold(0_u32, |total, _| total.saturating_add(CONTEXT_BONUS))
}

/// Entry names matching `query`, best first; ties break by name.
#[must_use]
pub fn rank(labels: &[String], query: &str, context: &SearchContext) -> Vec<String> {
    let mut scored: Vec<(u32, &String)> = labels
        .iter()
        .filter_map(|label| {
            title_score(label, query)
                .map(|title| (title.saturating_add(context_score(label, context)), label))
        })
        .collect();
    scored.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(right.1)));
    scored.into_iter().map(|(_, label)| label.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::{SearchContext, rank};
    use proptest::prelude::*;

    fn names(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    #[test]
    fn exact_title_beats_prefix_beats_word_match() {
        let labels = names(&["Work mail", "Mailbox", "mail"]);
        assert_eq!(
            rank(&labels, "mail", &SearchContext::default()),
            names(&["mail", "Mailbox", "Work mail"])
        );
    }

    #[test]
    fn spoken_phrase_containing_the_name_matches() {
        let labels = names(&["bank", "mail"]);
        assert_eq!(
            rank(&labels, "my bank login", &SearchContext::default()),
            names(&["bank"])
        );
    }

    #[test]
    fn non_matching_entries_are_left_out() {
        let labels = names(&["bank"]);
        assert_eq!(
            rank(&labels, "zebra", &SearchContext::default()),
            Vec::<String>::new()
        );
    }

    #[test]
    fn the_focused_app_breaks_a_tie() {
        let labels = names(&["Alpha mail", "Zulu mail"]);
        let context = SearchContext {
            app: Some("zulu".into()),
            field: None,
        };
        assert_eq!(
            rank(&labels, "mail", &context),
            names(&["Zulu mail", "Alpha mail"])
        );
    }

    #[test]
    fn the_focused_field_breaks_a_tie() {
        let labels = names(&["Card pin", "Card number"]);
        let context = SearchContext {
            app: None,
            field: Some("number".into()),
        };
        assert_eq!(
            rank(&labels, "card", &context),
            names(&["Card number", "Card pin"])
        );
    }

    proptest! {
        /// Ranking only reorders and filters: every result is an input label,
        /// with no duplicates introduced.
        #[test]
        fn results_are_a_subset_of_the_labels(
            labels in proptest::collection::vec("[a-z ]{0,8}", 0..8),
            query in "[a-z ]{0,6}",
            app in proptest::option::of("[a-z]{0,4}"),
        ) {
            let context = SearchContext { app, field: None };
            let found = rank(&labels, &query, &context);
            prop_assert!(found.len() <= labels.len());
            for label in &found {
                prop_assert!(labels.contains(label));
            }
        }

        /// An exact title always outranks every other entry, whatever the context.
        #[test]
        fn an_exact_title_is_first(
            others in proptest::collection::vec("[a-z]{1,6} [a-z]{1,6}", 0..6),
            app in proptest::option::of("[a-z]{1,4}"),
        ) {
            let mut labels = others;
            labels.push("target".to_owned());
            let context = SearchContext { app, field: None };
            let found = rank(&labels, "target", &context);
            prop_assert_eq!(found.first().map(String::as_str), Some("target"));
        }
    }
}
