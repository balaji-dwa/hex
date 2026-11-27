//! Slippy in the window and the capture capsule shows only for a signed-in
//! team member with Labs › Team sign-in on, and does no work otherwise.

use std::path::Path;

use super::{capsule, history_action, member, visible};
use crate::app_settings::AppSettings;
use crate::history::HistoryEntry;
use crate::rune_ui::capture_capsule::flow::{Done, Lead};
use crate::rune_ui::{Pane, Service, Ui};
use crate::team_auth::AuthState;

fn ui(lab: bool, state: AuthState) -> Ui {
    let mut settings = AppSettings::default();
    settings.labs.team_auth = lab;
    let mut ui = Ui::with(settings, Service::Fixture(Vec::new()));
    ui.lab_store.team_auth.state = state;
    ui
}

fn signed_in(email: &str) -> AuthState {
    AuthState::SignedIn {
        email: email.into(),
        member_sub: "member-sub".into(),
        expires_at_ms: None,
    }
}

fn entry(text: &str) -> HistoryEntry {
    serde_json::from_value(serde_json::json!({
        "id": 7,
        "timestamp_ms": 0,
        "kind": "dictation",
        "raw_text": text,
        "final_text": text,
    }))
    .unwrap()
}

fn paint() -> crate::rune_ui::controls::Paint {
    crate::rune_ui::controls::Paint::new(false, 2.0)
}

#[test]
fn the_pane_and_history_action_need_the_lab_and_a_signed_in_member() {
    let off = [
        ui(false, AuthState::SignedOut),
        ui(false, signed_in("nick@dwa1.com")),
        ui(true, AuthState::SignedOut),
        ui(
            true,
            AuthState::NeedsReauth {
                last_email: Some("nick@dwa1.com".into()),
                reason: String::new(),
            },
        ),
        ui(true, signed_in("nick@example.com")),
        ui(true, signed_in("@dwa1.com")),
    ];
    for ui in &off {
        assert_eq!(member(ui), None);
        assert!(!visible(ui));
        assert!(!Pane::Slippy.shown(ui));
        assert!(!crate::rune_ui::labs::main_panes(ui).contains(&Pane::Slippy));
        assert!(history_action(ui, paint(), &entry("hello")).is_none());
        assert!(!super::unread(ui));
    }

    let on = ui(true, signed_in("Nick@DWA1.com"));
    assert_eq!(member(&on), Some("member-sub"));
    assert!(Pane::Slippy.shown(&on));
    assert!(crate::rune_ui::labs::main_panes(&on).contains(&Pane::Slippy));
    assert!(history_action(&on, paint(), &entry("hello")).is_some());
    // Nothing to send: no button.
    assert!(history_action(&on, paint(), &entry("  ")).is_none());
}

#[test]
fn a_signed_out_window_starts_no_worker() {
    let mut ui = ui(true, AuthState::SignedOut);
    assert!(!super::poll(&mut ui, &mut rune::Tx::default()));
    assert!(!crate::slippy::active());
    assert!(!super::offered());
    assert!(ui.lab_store.slippy.snapshot.tasks.is_empty());
}

#[test]
fn slippy_links_name_one_task() {
    let parse = |link: &str| super::parse_link(&url::Url::parse(link).unwrap());
    assert_eq!(
        parse("hex://slippy/1700000000.000001").as_deref(),
        Ok("1700000000.000001")
    );
    for link in [
        "hex://slippy/",
        "hex://slippy/../labs",
        "hex://slippy/1700000000.000001?x=1",
        "hex://slippy/1700000000.000001#top",
        "hex://user@slippy/1700000000.000001",
        "hex://slippy/1700000000.000001/extra",
    ] {
        assert!(parse(link).is_err(), "{link}");
    }
}

#[test]
fn the_capsule_offers_slippy_only_while_signed_in() {
    let bundle = Path::new("/tmp/take.hexannotation");
    let mut done = Done::plain(Lead::Check, "Annotation");
    capsule::test_apply(&mut done, false, Some(bundle));
    assert_eq!(done.slippy, None);
    capsule::test_apply(&mut done, true, None);
    assert_eq!(done.slippy, None);
    capsule::test_apply(&mut done, true, Some(bundle));
    assert_eq!(done.slippy, Some("Slippy"));
    // Never over the OpenCode list or the Always… rule popover.
    done.open = Some((rune::TextEdit::default(), empty_list()));
    capsule::test_apply(&mut done, true, Some(bundle));
    assert_eq!(done.slippy, None);

    assert_eq!(capsule::test_label(false, None), "Slippy");
    assert_eq!(capsule::test_label(true, None), "Sending…");
    assert_eq!(capsule::test_label(false, Some(true)), super::SENT);
    assert_eq!(capsule::test_label(false, Some(false)), "Couldn't send");

    // The real hook reads the account: with none, no button.
    let mut pres =
        crate::rune_ui::capture_capsule::flow::Pres::Done(Done::plain(Lead::Check, "Annotation"));
    capsule::decorate(&mut pres, Some(&bundle.to_path_buf()));
    let crate::rune_ui::capture_capsule::flow::Pres::Done(done) = pres else {
        unreachable!()
    };
    assert_eq!(done.slippy, None);
    assert!(!capsule::pending());
}

fn empty_list() -> crate::rune_ui::capture_capsule::flow::List {
    crate::rune_ui::capture_capsule::flow::List {
        rows: Vec::new(),
        title: "",
        min_rows: 0,
        selected: 0,
        target: None,
        footer: crate::rune_ui::capture_capsule::flow::Footer::Suggesting,
        always: None,
        queued: false,
    }
}
