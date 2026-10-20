//! The Whisper prompt vocabulary keeps explicit words first and stays under its cap.

#[cfg(test)]
mod capture_vocabulary {
    use proptest::prelude::*;
    use tehuti::contexts::{VOCABULARY_WORDS, capture_vocabulary};

    fn words(phrases: &[String]) -> usize {
        phrases
            .iter()
            .map(|phrase| phrase.split_whitespace().count())
            .sum()
    }

    proptest! {
        #[test]
        fn explicit_words_come_first_and_the_cap_holds(
            explicit in prop::collection::vec("[a-z]{1,8}( [a-z]{1,8}){0,2}", 0..80),
            apps in prop::collection::vec("[A-Z][a-z]{1,8}( [a-z]{1,8}){0,2}", 0..80),
        ) {
            let vocabulary = capture_vocabulary(explicit.clone(), apps.clone());
            prop_assert!(words(&vocabulary) <= VOCABULARY_WORDS);
            let first_app = vocabulary.iter().position(|phrase| apps.contains(phrase));
            if let Some(first_app) = first_app {
                prop_assert!(vocabulary[first_app..].iter().all(|phrase| apps.contains(phrase)));
            }
            let unique: std::collections::BTreeSet<_> = vocabulary.iter().collect();
            prop_assert_eq!(unique.len(), vocabulary.len());
            let mut deduped = explicit;
            deduped.sort();
            deduped.dedup();
            if words(&deduped) <= VOCABULARY_WORDS {
                for phrase in &deduped {
                    prop_assert!(vocabulary.contains(phrase), "explicit {} dropped", phrase);
                }
            }
        }
    }

    #[test]
    fn app_names_fill_only_the_room_explicit_words_leave() {
        let explicit: Vec<String> = (0..VOCABULARY_WORDS - 1)
            .map(|index| format!("word{index}"))
            .collect();
        let vocabulary = capture_vocabulary(
            explicit.clone(),
            ["Web Browser".to_owned(), "Atlas".to_owned(), "Notes".to_owned()],
        );
        assert_eq!(vocabulary[..explicit.len()], explicit[..]);
        assert_eq!(vocabulary[explicit.len()..], ["Atlas".to_owned()]);
    }
}
