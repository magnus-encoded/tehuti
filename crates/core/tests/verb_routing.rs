//! First-word verb routing through the pure core seam.
#[cfg(test)]
mod verb_routing {
    use proptest::prelude::*;
    use tehuti_core::verbs::{VerbMatch, route};
    use tehuti_protocol::Text;

    fn owners(verbs: &[(usize, Vec<String>)]) -> impl Iterator<Item = (usize, &[String])> {
        verbs.iter().map(|(plugin, list)| (*plugin, list.as_slice()))
    }

    fn verb() -> impl Strategy<Value = String> {
        "[a-z]{3,10}"
    }

    fn remainder() -> impl Strategy<Value = String> {
        "[a-z]{1,8}( [a-z]{1,8}){0,4}"
    }

    proptest! {
        #[test]
        fn a_spoken_verb_reaches_its_owner_with_the_rest_of_the_words(
            verbs in prop::collection::vec(verb(), 1..6),
            pick in any::<prop::sample::Index>(),
            rest in remainder(),
            punctuation in prop::sample::select(vec!["", ",", ".", ":"]),
        ) {
            let spoken = pick.get(&verbs).clone();
            let table = [(7, verbs.clone())];
            let matched = route(owners(&table), &format!("{spoken}{punctuation} {rest}"));
            let matched = matched.expect("a spoken verb routes");
            prop_assert_eq!(matched.plugin, 7, "only one owner exists");
            prop_assert_eq!(matched.text.as_str(), rest.as_str(), "the verb is stripped");
            prop_assert!(verbs.contains(&matched.verb), "the verb is one the plugin owns");
        }

        #[test]
        fn speech_without_a_remainder_or_with_no_verbs_matches_nothing(
            verbs in prop::collection::vec(verb(), 0..6),
            pick in any::<prop::sample::Index>(),
            rest in remainder(),
        ) {
            let table = [(0, verbs.clone())];
            if let Some(spoken) = verbs.get(pick.index(verbs.len().max(1))) {
                prop_assert_eq!(route(owners(&table), spoken), None, "a lone verb falls through");
                prop_assert_eq!(route(owners(&table), &format!("{spoken}   ")), None, "trailing spaces are no remainder");
            }
            prop_assert_eq!(route(owners(&[(0, Vec::new())]), &format!("word {rest}")), None, "no verbs, no route");
        }

        #[test]
        fn the_earlier_owner_wins_a_shared_verb(shared in verb(), rest in remainder(), first in 0_usize..4) {
            let table = [(first, vec![shared.clone()]), (9, vec![shared.clone()])];
            let matched = route(owners(&table), &format!("{shared} {rest}")).expect("shared verb routes");
            prop_assert_eq!(matched.plugin, first, "manifest order breaks ties");
        }

        #[test]
        fn routing_never_invents_text(transcript in ".{0,60}", verbs in prop::collection::vec(verb(), 0..4)) {
            let table = [(0, verbs)];
            if let Some(matched) = route(owners(&table), &transcript) {
                prop_assert!(transcript.ends_with(matched.text.as_str()), "the remainder is a suffix");
                prop_assert!(!matched.text.as_str().trim().is_empty(), "the remainder has words");
            }
        }
    }

    #[test]
    fn sound_folding_catches_misheard_verbs_but_not_other_words() {
        let table = [(0, vec!["invoke".to_owned()]), (1, vec!["ada".to_owned()])];
        let heard = |text: &str| route(owners(&table), text);
        assert_eq!(
            heard("Invoce usage report"),
            Some(VerbMatch {
                plugin: 0,
                verb: "invoke".into(),
                text: Text::new("usage report".into()).unwrap(),
            }),
            "a folded spelling matches"
        );
        assert_eq!(
            heard("Ada, summarise the reviews").map(|found| found.text),
            Some(Text::new("summarise the reviews".into()).unwrap()),
            "comma is stripped"
        );
        assert_eq!(heard("Adam was here"), None, "short verbs need equal sound keys");
        assert_eq!(heard("open the file"), None, "other first words fall through");
        assert_eq!(heard("please invoke usage"), None, "only the first word counts");
    }
}
