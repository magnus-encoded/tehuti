//! Laws of the fuzzy speech key and its distance.

#[cfg(test)]
mod fuzzy {
    use proptest::prelude::*;
    use tehuti_protocol::{damerau_levenshtein, fold_phrase, joined_key, parse_context, similarity};

    #[test]
    fn norwegian_spellings_fold_together() {
        for (left, right) in [
            ("Åpne Pokémon Rød.", "Åpne Pokemon Rød"),
            ("Åpner Pokémon Gull.", "åpne pokemon gul"),
            ("Lukke vinduet!", "Lukk vindu"),
            ("Hva?", "va"),
            ("Pokémon-rødet", "pokemon rod"),
            ("Bærekraft", "berekraft"),
        ] {
            assert_eq!(fold_phrase(left), fold_phrase(right), "{left} and {right}");
        }
        assert_ne!(
            fold_phrase("Pokémon Gull"),
            fold_phrase("Pokémon Rød"),
            "names stay apart"
        );
    }

    #[test]
    fn split_and_joined_words_share_a_joined_key() {
        for (left, right) in [
            ("Super. Tux.", "SuperTux."),
            ("Super-Tux", "Super Tux"),
            ("Åpne Super Tux", "åpne supertux"),
        ] {
            assert_eq!(joined_key(left), joined_key(right), "{left} and {right}");
        }
    }

    #[test]
    fn a_swap_of_neighbours_costs_one_edit() {
        assert_eq!(damerau_levenshtein("pokemon", "pokmeon"), 1, "swap");
        assert_eq!(damerau_levenshtein("pokemon", "pokeman"), 1, "substitution");
        assert_eq!(damerau_levenshtein("", "abc"), 3, "insertions");
    }

    #[test]
    fn the_fuzzy_threshold_is_a_fraction_between_zero_and_one() {
        for fuzzy in ["-0.1", "1.5", "'high'", "nan", "inf"] {
            assert!(
                parse_context(&format!(
                    "app_id = 'a'\n[buttons]\n[speech_matching]\nfuzzy = {fuzzy}"
                ))
                .is_err(),
                "{fuzzy}"
            );
        }
        let context = parse_context("app_id = 'a'\n[buttons]\n[speech_matching]\nfuzzy = 0.85").unwrap();
        assert_eq!(
            context.speech_matching().fuzzy.unwrap().thousandths(),
            850,
            "thousandths"
        );
    }

    proptest! {
        #[test]
        fn similarity_is_symmetric_bounded_and_whole_for_equal_keys(left in "[a-zøæå ]{0,20}", right in "[a-zøæå ]{0,20}") {
            prop_assert_eq!(similarity(&left, &right), similarity(&right, &left));
            prop_assert!(similarity(&left, &right) <= 1000);
            prop_assert_eq!(similarity(&left, &left), 1000);
            prop_assert!(damerau_levenshtein(&left, &right) <= left.chars().count().max(right.chars().count()));
        }

        #[test]
        fn folding_ignores_case_and_punctuation(phrase in "[A-Za-zÆØÅæøå ]{1,20}") {
            prop_assert_eq!(fold_phrase(&phrase), fold_phrase(&format!("{}.", phrase.to_uppercase())));
        }
    }

    #[test]
    fn boolean_opt_in_uses_eight_tenths_and_false_disables_matching() {
        for (setting, expected) in [("true", 800), ("false", 0)] {
            let context = tehuti_protocol::parse_context(&format!(
                "app_id = 'editor'\n[buttons]\n[speech_matching]\nfuzzy = {setting}"
            ))
            .unwrap();
            assert_eq!(context.speech_matching().fuzzy.unwrap().thousandths(), expected);
        }
    }
}
