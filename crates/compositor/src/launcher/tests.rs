use super::{InvitationView, Session};
use crate::destinations::{Target, Window};
use core::time::Duration;
use std::time::Instant;
use tehuti_core::unit_verbs::DestinationKind;

fn session() -> Session {
    Session::new().expect("entry worker starts")
}

fn window(identity: u64) -> Window {
    Window {
        identity,
        app_id: "editor".into(),
        title: "Document".into(),
        chip: "Editor".into(),
    }
}

#[test]
fn rows_select_the_corresponding_live_window_route() {
    let mut launcher = session();
    assert!(launcher.update_windows(vec![window(42)]));
    assert!(!launcher.update_windows(vec![window(42)]));
    let rows = launcher.rows(DestinationKind::Application, None);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "Editor");
    let route = launcher.choose(0, false, Instant::now()).expect("window route");
    assert_eq!(route.steps[0].target, Target::Window(42));
    assert!(launcher.choose(1, true, Instant::now()).is_none());
    assert!(launcher.update_windows(Vec::new()));
    assert_eq!(launcher.rows(DestinationKind::Application, None).len(), 0);
}

#[test]
fn invitation_requires_enablement_an_empty_desktop_and_no_overlay() {
    let mut launcher = session();
    let now = Instant::now();
    assert_eq!(launcher.invitation(false, false, now), InvitationView::Hidden);
    launcher.enable_invitation();
    assert_eq!(launcher.invitation(false, true, now), InvitationView::Hidden);
    assert_eq!(launcher.invitation(true, false, now), InvitationView::Close);
    assert_eq!(launcher.invitation(false, false, now), InvitationView::Show);
}

#[test]
fn invitation_waits_ten_seconds_after_selection_and_failure_releases_it() {
    let mut launcher = session();
    let now = Instant::now();
    launcher.enable_invitation();
    assert!(launcher.update_windows(vec![window(1)]));
    let _rows = launcher.rows(DestinationKind::Application, None);
    assert!(launcher.choose(0, true, now).is_some());
    assert_eq!(
        launcher.invitation(false, false, now + Duration::from_secs(9)),
        InvitationView::Hidden
    );
    assert_eq!(
        launcher.invitation(false, false, now + Duration::from_secs(10)),
        InvitationView::Show
    );
    assert!(launcher.route(0).is_none());
    let _refreshed_rows = launcher.rows(DestinationKind::Application, None);
    assert!(launcher.choose(0, true, now).is_some());
    launcher.launch_failed();
    assert_eq!(launcher.invitation(false, false, now), InvitationView::Show);
}

#[test]
fn close_resets_launching_during_the_grace() {
    let mut launcher = session();
    let now = Instant::now();
    launcher.enable_invitation();
    assert!(launcher.update_windows(vec![window(1)]));
    let _rows = launcher.rows(DestinationKind::Application, None);
    assert!(launcher.choose(0, true, now).is_some());
    assert_eq!(
        launcher.invitation(false, false, now + Duration::from_secs(1)),
        InvitationView::Hidden
    );
    assert_eq!(
        launcher.invitation(true, false, now + Duration::from_secs(2)),
        InvitationView::Close
    );
    assert_eq!(
        launcher.invitation(false, false, now + Duration::from_secs(3)),
        InvitationView::Show
    );
}
