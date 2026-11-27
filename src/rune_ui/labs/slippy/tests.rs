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

const READ: &str = "991795815730.000001";
const FRESH: &str = "991795815730.000002";

fn task(ts: &str, status: &str) -> crate::slippy::Task {
    serde_json::from_value(serde_json::json!({
        "ts": ts,
        "origin": "annotate",
        "title": format!("Task {ts}"),
        "status": status,
        "seq": 1,
        "createdAt": 1_700_000_000_000_u64,
        "updatedAt": 1_700_000_000_000_u64,
    }))
    .unwrap()
}

fn thumbnail_png() -> Vec<u8> {
    image_png(320, 200)
}

fn image_png(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let pixels: Vec<u8> = (0..width * height)
        .flat_map(|index| {
            let (x, y) = (index % width, index / width);
            [(x * 255 / width) as u8, (y * 255 / height) as u8, 160]
        })
        .collect();
    encoder
        .write_header()
        .unwrap()
        .write_image_data(&pixels)
        .unwrap();
    bytes
}

/// The Slippy pane over a read and an unread finished task, the read one
/// open with an image and a text attachment, its image decoded.
fn slippy_runtime() -> rune::Runtime<Ui> {
    let mut ui = ui(true, signed_in("nick@dwa1.com"));
    ui.pane = Pane::Slippy;
    let data = &mut ui.lab_store.slippy;
    data.snapshot.tasks = vec![task(READ, "done"), task(FRESH, "done")];
    data.snapshot.unread.insert(FRESH.into());
    data.snapshot.selection = Some(READ.into());
    data.snapshot.detail = Some(Ok(serde_json::from_value(serde_json::json!({
        "task": task_json(READ),
        "messages": [{
            "ts": READ,
            "role": "user",
            "text": "Fix this",
            "files": [
                {"id": "F1", "name": "annotation.png", "mimeType": "image/png"},
                {"id": "F2", "name": "notes.txt", "mimeType": "text/plain"},
            ],
        }],
    }))
    .unwrap()));
    data.snapshot.thumbs.insert(
        (READ.into(), "F1".into()),
        Some(std::sync::Arc::from(thumbnail_png())),
    );
    assert!(data.sync_images(Some(std::sync::Arc::new(|| {}))));
    data.decoded[&(READ.to_owned(), "F1".to_owned())]
        .wait()
        .unwrap();
    let env = rune::Env {
        system_dark: false,
        scale: 2.0,
    };
    let text =
        rune::text::TextSystem::new(rune_theme_opencode::FONTS, rune_theme_opencode::FAMILIES);
    let mut rt =
        crate::rune_ui::offscreen::runtime(ui, crate::rune_ui::view, (980.0, 760.0), env, text);
    rt.on_key = Some(crate::rune_ui::on_key);
    settle(&mut rt);
    rt
}

fn task_json(ts: &str) -> serde_json::Value {
    serde_json::json!({
        "ts": ts, "origin": "annotate", "title": format!("Task {ts}"), "status": "done",
        "seq": 1, "createdAt": 1_700_000_000_000_u64, "updatedAt": 1_700_000_000_000_u64,
    })
}

fn settle(rt: &mut rune::Runtime<Ui>) {
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
}

fn tap(rt: &mut rune::Runtime<Ui>, key: &str) {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no element {key}"));
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(rt);
}

fn under(rt: &rune::Runtime<Ui>, root: &str) -> Vec<rune::ElementInfo> {
    rt.elements()
        .into_iter()
        .filter(|e| !e.exiting && e.path.contains(root) && !e.path.contains("/faces/size"))
        .collect()
}

fn shown(rt: &rune::Runtime<Ui>, key: &str) -> bool {
    rt.elements()
        .iter()
        .any(|e| !e.exiting && e.key.as_deref() == Some(key))
}

#[test]
fn finished_tasks_drop_done_and_unread_text_and_center_row_content() {
    let p = paint();
    for status in [
        crate::slippy::Status::Queued,
        crate::slippy::Status::Running,
        crate::slippy::Status::Waiting,
        crate::slippy::Status::Failed,
    ] {
        for unread in [false, true] {
            let (label, ink) = super::pane::row_status(p, status, unread);
            assert_eq!(label, status.label());
            assert!(ink.is_some());
        }
    }
    let done = crate::slippy::Status::Done;
    assert_eq!(super::pane::row_status(p, done, false), ("", None));
    assert_eq!(
        super::pane::row_status(p, done, true),
        ("", Some(p.accent()))
    );

    let rt = slippy_runtime();
    let read_key = format!("slippy-task-{READ}");
    let fresh_key = format!("slippy-task-{FRESH}");
    let read = under(&rt, &read_key);
    let fresh = under(&rt, &fresh_key);
    let texts = |elements: &[rune::ElementInfo]| -> Vec<String> {
        elements.iter().filter_map(|e| e.text.clone()).collect()
    };
    fn find_suffix<'a>(elements: &'a [rune::ElementInfo], suffix: &str) -> &'a rune::ElementInfo {
        elements
            .iter()
            .find(|e| e.path.ends_with(suffix))
            .unwrap_or_else(|| panic!("no element ending with {suffix}"))
    }
    let center_y = |e: &rune::ElementInfo| e.target[1] + e.target[3] / 2.0;

    // Read finished row: no status text, no dot, title & time centered in the row.
    assert!(
        !texts(&read)
            .iter()
            .any(|text| text == "Done" || text == "Unread"),
        "{:?}",
        texts(&read)
    );
    assert!(!read.iter().any(|e| e.path.ends_with("/dot")));
    let read_box = find_suffix(&read, &read_key);
    let read_title = find_suffix(&read, "/title");
    let read_time = find_suffix(&read, "/time");
    assert!(
        (center_y(read_title) - center_y(read_box)).abs() <= 1.0,
        "read title not centered: title={} box={}",
        center_y(read_title),
        center_y(read_box)
    );
    assert!(
        (center_y(read_time) - center_y(read_box)).abs() <= 1.0,
        "read time not centered: time={} box={}",
        center_y(read_time),
        center_y(read_box)
    );

    // Unread finished row: neither "Done" nor "Unread" text, only the dot,
    // and title, time, and dot all centered in the row box.
    assert!(
        !texts(&fresh)
            .iter()
            .any(|text| text == "Done" || text == "Unread"),
        "{:?}",
        texts(&fresh)
    );
    let fresh_box = find_suffix(&fresh, &fresh_key);
    let fresh_title = find_suffix(&fresh, "/title");
    let fresh_time = find_suffix(&fresh, "/time");
    let fresh_dot = find_suffix(&fresh, "/dot");
    for (name, el) in [
        ("title", fresh_title),
        ("time", fresh_time),
        ("dot", fresh_dot),
    ] {
        assert!(
            (center_y(el) - center_y(fresh_box)).abs() <= 1.0,
            "fresh {name} not centered: {name}={} box={}",
            center_y(el),
            center_y(fresh_box)
        );
    }

    // The open, finished thread's header carries only its title.
    assert!(
        !texts(&under(&rt, "detail-header"))
            .iter()
            .any(|text| text == "Done")
    );
}

#[test]
fn an_image_attachment_is_a_thumbnail_that_opens_a_preview() {
    let mut rt = slippy_runtime();
    let key = (READ.to_owned(), "F1".to_owned());
    assert!(shown(&rt, "slippy-image-F1"));
    // Not an image: its name.
    assert!(
        rt.elements()
            .iter()
            .any(|e| e.text.as_deref() == Some("notes.txt"))
    );
    assert!(!shown(&rt, "slippy-preview"));

    tap(&mut rt, "slippy-image-F1");
    assert_eq!(rt.state.lab_store.slippy.preview.as_ref(), Some(&key));
    assert!(shown(&rt, "slippy-preview"));
    assert!(shown(&rt, "slippy-preview-image"));
    rt.key(rune::KeyEvent {
        key: rune::Key::Escape,
        mods: rune::Mods::default(),
    });
    settle(&mut rt);
    assert_eq!(rt.state.lab_store.slippy.preview, None, "Escape closes it");
    assert!(!shown(&rt, "slippy-preview"));

    tap(&mut rt, "slippy-image-F1");
    tap(&mut rt, "slippy-preview-close");
    assert_eq!(rt.state.lab_store.slippy.preview, None, "Close closes it");

    tap(&mut rt, "slippy-image-F1");
    rt.pointer_down(4.0, 4.0);
    rt.pointer_up(4.0, 4.0);
    settle(&mut rt);
    assert_eq!(
        rt.state.lab_store.slippy.preview, None,
        "a press outside closes it"
    );

    // The worker let the image go (sign-out, another account): no preview.
    tap(&mut rt, "slippy-image-F1");
    rt.state.lab_store.slippy.snapshot.thumbs.clear();
    assert!(rt.state.lab_store.slippy.sync_images(None));
    assert_eq!(rt.state.lab_store.slippy.preview, None);
    assert!(rt.state.lab_store.slippy.decoded.is_empty());
}

fn size_of(rt: &rune::Runtime<Ui>, key: &str) -> (f32, f32) {
    let element = rt
        .elements()
        .into_iter()
        .find(|e| !e.exiting && e.key.as_deref() == Some(key))
        .unwrap_or_else(|| panic!("no element {key}"));
    (element.target[2], element.target[3])
}

#[test]
fn the_preview_decodes_the_full_image_only_while_open() {
    let mut rt = slippy_runtime();
    let key = (READ.to_owned(), "F1".to_owned());
    // The thread decodes only the thumbnail.
    assert!(rt.state.lab_store.slippy.full.is_none());

    tap(&mut rt, "slippy-image-F1");
    assert_eq!(rt.state.lab_store.slippy.preview.as_ref(), Some(&key));
    // Until the full image arrives, the thumbnail fills the preview.
    assert!(shown(&rt, "slippy-preview-image"));
    assert!(rt.state.lab_store.slippy.full.is_none());

    // The worker loaded it: decoded once, drawn at its own size.
    let data = &mut rt.state.lab_store.slippy;
    data.snapshot.preview_image = Some((key.clone(), std::sync::Arc::from(image_png(800, 500))));
    assert!(data.sync_images(Some(std::sync::Arc::new(|| {}))));
    data.full.as_ref().unwrap().1.wait().unwrap();
    assert!(!data.sync_images(None), "decoded once");
    rt.invalidate();
    settle(&mut rt);
    assert_eq!(size_of(&rt, "slippy-preview-image"), (400.0, 250.0));

    // Closed: the full-size decode goes with it.
    rt.key(rune::KeyEvent {
        key: rune::Key::Escape,
        mods: rune::Mods::default(),
    });
    settle(&mut rt);
    assert_eq!(rt.state.lab_store.slippy.preview, None);
    assert!(rt.state.lab_store.slippy.full.is_none());
    // A full image for a preview no longer open is never decoded.
    assert!(!rt.state.lab_store.slippy.sync_images(None));
    assert!(rt.state.lab_store.slippy.full.is_none());
}
