#[cfg(test)]
mod tests {
    use tehuti_core::choice_list::{Candidate, Choice, List, Slots};

    fn slots() -> Slots {
        Slots {
            title: "Choose".into(),
            heard: "orbytt".into(),
            scopes: vec!["Examples".into()],
            create: None,
            candidates: ["zebra", "orbit", "alpha", "orbyt"]
                .into_iter()
                .map(|name| Candidate {
                    name: name.into(),
                    meta: None,
                })
                .collect(),
            action: "Open".into(),
            closest_heading: "CLOSEST".into(),
            remaining_heading: "ALL ITEMS".into(),
        }
    }

    #[test]
    fn near_matches_rank_by_distance_and_the_rest_alphabetically() {
        let list = List::new(slots());
        let names: Vec<_> = list
            .candidates()
            .iter()
            .map(|row| row.candidate.name.as_str())
            .collect();
        assert_eq!(names, ["orbyt", "orbit", "alpha", "zebra"]);
        assert_eq!(list.selected(), Some(Choice::Candidate(3)));
        assert!(list.show_headings());
    }
    #[test]
    fn hints_follow_roles_and_disabled_create_cannot_be_confirmed() {
        use tehuti_core::{
            choice_list::{Create, Outcome},
            pad::{PadFamily, Role},
        };
        let mut supplied = slots();
        supplied.heard = "new item".into();
        supplied.create = Some(Create {
            heading: "+ NEW ITEM".into(),
            detail: "Name unavailable".into(),
            action: "Create".into(),
            enabled: false,
        });
        let mut list = List::new(supplied);
        assert_eq!(list.selected(), Some(Choice::Create));
        assert_eq!(list.selected_hint(PadFamily::Nintendo), None);
        list.confirm();
        assert_eq!(list.outcome(), Outcome::Pending);
        list.move_selection(true);
        for family in [PadFamily::Letters, PadFamily::Symbols, PadFamily::Nintendo] {
            assert_eq!(
                list.selected_hint(family),
                Some((family.glyph(Role::Confirm), "Open"))
            );
            assert_eq!(list.back_hint(family), (family.glyph(Role::Back), "Close"));
        }
    }

    #[test]
    fn prefix_spans_are_case_insensitive_and_unicode_safe() {
        let mut supplied = slots();
        supplied.heard = "ÉCHx".into();
        supplied.candidates = vec![
            Candidate {
                name: "écho".into(),
                meta: None,
            },
            Candidate {
                name: "distant".into(),
                meta: None,
            },
        ];
        let list = List::new(supplied);
        assert_eq!(list.candidates()[0].name_spans(), ("éch", "o"));
        assert_eq!(list.candidates()[1].name_spans(), ("distant", ""));
    }

    #[test]
    fn candidate_window_tracks_selection_and_navigation_stops_at_ends() {
        let mut supplied = slots();
        supplied.heard.clear();
        supplied.candidates = (0..9)
            .map(|index| Candidate {
                name: format!("Item {index}"),
                meta: None,
            })
            .collect();
        let mut list = List::new(supplied);
        list.move_selection(false);
        assert_eq!(list.selected(), Some(Choice::Candidate(0)));
        for _ in 0..20 {
            list.move_selection(true);
        }
        assert_eq!(list.selected(), Some(Choice::Candidate(8)));
        assert_eq!(list.visible(), 4..9);
        for _ in 0..20 {
            list.move_selection(false);
        }
        assert_eq!(list.visible(), 0..5);
        list.confirm();
        assert_eq!(
            list.outcome(),
            tehuti_core::choice_list::Outcome::Picked(Choice::Candidate(0))
        );
    }

    #[test]
    fn empty_lists_have_no_selection_or_confirm_hint() {
        let mut supplied = slots();
        supplied.candidates.clear();
        let mut list = List::new(supplied);
        list.move_selection(true);
        list.confirm();
        assert_eq!(list.selected(), None);
        assert_eq!(list.selected_hint(tehuti_core::pad::PadFamily::Letters), None);
        assert!(!list.show_headings());
        list.cancel();
        assert!(!list.is_open());
    }
    #[test]
    fn create_is_default_only_without_a_near_match_and_confirm_retains_the_choice() {
        use tehuti_core::choice_list::{Create, Outcome};
        let mut supplied = slots();
        supplied.create = Some(Create {
            heading: "+ NEW ITEM".into(),
            detail: "Use draft".into(),
            action: "Create".into(),
            enabled: true,
        });
        let near = List::new(supplied.clone());
        assert_eq!(near.selected(), Some(Choice::Candidate(3)));
        supplied.heard = "a completely different name".into();
        let mut list = List::new(supplied);
        assert_eq!(list.selected(), Some(Choice::Create));
        assert_eq!(
            list.selected_hint(tehuti_core::pad::PadFamily::Letters)
                .map(|(_, text)| text),
            Some("Create")
        );
        list.confirm();
        list.cancel();
        assert_eq!(list.outcome(), Outcome::Picked(Choice::Create));
        assert_eq!(list.selected_hint(tehuti_core::pad::PadFamily::Letters), None);
    }

    #[test]
    fn near_threshold_and_distance_ties_have_a_stable_alphabetical_order() {
        let list = List::new(Slots {
            heard: "abcd".into(),
            candidates: ["zzzz", "xbcd", "aXXX", "AbCd", "Xbcd", "abcx", "aaaaaa"]
                .into_iter()
                .map(|name| Candidate {
                    name: name.into(),
                    meta: None,
                })
                .collect(),
            ..Slots::default()
        });
        let rows: Vec<_> = list
            .candidates()
            .iter()
            .map(|row| (row.choice, row.close))
            .collect();
        assert_eq!(
            rows,
            [
                (Choice::Candidate(3), true),
                (Choice::Candidate(5), true),
                (Choice::Candidate(1), true),
                (Choice::Candidate(4), true),
                (Choice::Candidate(2), true),
                (Choice::Candidate(6), false),
                (Choice::Candidate(0), false),
            ]
        );
    }

    proptest::proptest! {
        #[test]
        fn navigation_keeps_selection_visible_and_preserves_every_candidate(
            names in proptest::collection::vec("[a-zé]{0,12}", 0..24),
            heard in "[a-zé]{0,12}",
            create in proptest::bool::ANY,
            moves in proptest::collection::vec(proptest::bool::ANY, 0..80),
        ) {
            use proptest::prelude::*;
            let supplied = Slots {
                heard,
                candidates: names.iter().map(|name| Candidate { name: name.clone(), meta: None }).collect(),
                create: create.then(|| tehuti_core::choice_list::Create {
                    heading: "New item".into(), detail: String::new(), action: "Create".into(), enabled: true,
                }),
                ..Slots::default()
            };
            let mut list = List::new(supplied);
            let mut identities: Vec<_> = list.candidates().iter().map(|row| match row.choice {
                Choice::Candidate(index) => index,
                Choice::Create => usize::MAX,
            }).collect();
            identities.sort_unstable();
            prop_assert_eq!(identities, (0..names.len()).collect::<Vec<_>>());
            for down in core::iter::once(false).chain(moves) {
                list.move_selection(down);
                prop_assert!(list.visible().len() <= 5);
                if let Some(Choice::Candidate(index)) = list.selected() {
                    prop_assert!(list.candidates()[list.visible()].iter().any(|row| row.choice == Choice::Candidate(index)));
                }
                for row in list.candidates() {
                    let (bright, tail) = row.name_spans();
                    prop_assert_eq!(format!("{bright}{tail}"), row.candidate.name.as_str());
                }
            }
        }
    }
}
