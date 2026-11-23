# Planner

Planner is a macOS-only, Everyone Lab, off by default (`labs.planner`).
Its ledger is local: `planner.json` in Hex's Application Support folder,
separate from dictation History and its retention settings. The planner
slides use English; the Lab card follows the interface language.

```ts
Labs › Planner on                                      // planner.opt-in
├── Add Planner to the sidebar -> Open planner anytime
├── Add Today’s Steps to Hex’s menu bar (checked while pinned)
├── Register Hex with macOS Launch at login
│   └── macOS requires approval / registration fails -> explain in Planner
└── Check at launch and once a second while Hex runs     // planner.schedule
    ├── Display asleep, Mac asleep, or session locked -> wait until return
    ├── Completed for this planning day -> stay hidden
    └── Due -> full-screen Rune panel on the pointer's display
        ├── Other displays -> ordinary native backdrops; follow display changes
        ├── Not now / Escape (outside an editor) -> save draft, snooze 5 min
        │   └── Deadline expires -> show again, unless completed or away
        └── System modal/security UI, menu bar, lock, Force Quit -> remain available
```

The day starts at **6 AM America/New_York**, including daylight-saving time,
independent of the Mac's current time zone. A saved five-minute snooze
survives app restarts. Hex must be running for the reminder to appear;
quitting remains available. Turning the Lab off stops presentation, hides
Today’s Steps, and keeps the ledger. Launch at login is the existing OS-owned
setting and remains on when Planner is turned off.

```ts
Daily planning                                         // planner.daily
├── Big picture -> goal, weekly focus, success criteria, remaining work,
│                  and recent 7-day rhythm strip
├── Yesterday’s check-in -> side-by-side Deliverable | Done when -> Yes, done / Not yet
│   ├── Yes, done -> saves step snapshot and marks yesterday’s steps done
│   ├── Not yet -> restores step snapshot and reveals optional blocker note
│   ├── Unchecking a step while Yes, done is active -> flips to Not yet
│   └── Missed days -> label the last plan with its actual date
├── Today -> choose one problem or piece
├── Outcome -> concrete deliverable AND explicit done-when criteria together
├── Break it into small steps -> today's focus & outcome strip above steps
│   ├── Unfinished yesterday steps -> Keep / Drop (or Keep all / Drop all) triage
│   └── > 5 steps -> gentle nudge to stay to 3–5 essentials
└── Commit -> Today’s plan recap & in-day checkable workspace (DayView)
    ├── Invalid answer or checklist -> stay on the slide, highlight & focus field
    ├── Disk save fails -> keep the draft; never mark completion
    ├── Check off steps in place -> saves immediately; Edit plan rolls back on Cancel/Esc
    └── Start the day / Done / Esc -> hide until the next planning day

Weekly review                                         // planner.weekly
├── First enable -> Goal (+ remaining work) -> Focus (+ success criteria)
└── Seven calendar days since the last completed review
    ├── Progress -> recent 7-day rhythm, delivered work, and weekly reflection
    ├── Goal -> keep or revise the overarching goal and remaining work
    └── Focus -> next week's focus and success criteria -> save -> daily flow
```

Open-ended answers and checklist steps require at least **three characters**
with a letter or number (concise answers are accepted without a surprise
five-word wall). Final completion checks the entire plan, not just the visible
slide. The previous deliverable needs an explicit Yes, done / Not yet choice
when a previous plan exists. A checklist entry waiting for Add must be added
or cleared before committing.

Saved answers wrap within the content column, including long words and links.
`planner_saved_answers_wrap_within_the_content_column` covers long summaries,
yesterday's deliverable/criteria, and the pinned focus at two viewport widths.
The card keeps consistent outer padding, card padding, and header vertical
position across slides, and both the Weekly review and Yesterday check-in (with
Not yet, blocker note, and steps) fit inside 1024×768 without clipping.

Keyboard navigation works without global interception:
- Outside an editor: `←` / `→` navigate (`←` on `DayView` enters `Edit plan`), `Enter` advances/commits.
- Inside an editor: `←` / `→` move the caret; first `Esc` exits the editor, second `Esc` cancels an edit (restoring `edit_backup`) or snoozes for 5 minutes (even when `planner.json` is unreadable).
- Multi-field slides (`Goal`, `Focus`, `Outcome`): `Enter` in the first field moves focus to the second field without inserting a newline; `Enter` in the second field or `⌘Enter` anywhere advances; `⇧Enter` inserts a newline.
- `Breakdown`: `Enter` in an existing step moves focus to `Add a step` (never skips the slide); `Enter` in a non-empty `Add a step` adds the step; `Enter` in an empty `Add a step` (when ≥ 1 step exists) or `⌘Enter` commits.
- Reopening a committed plan opens `DayView` directly; closing it does not schedule another reminder that day. On the first day, with no previous plan, `Yesterday` is skipped.

```ts
Today’s Steps floating overlay                         // planner.peek
├── Entry modes (resolved from dictation_hotkey + shortcut_mode)
│   ├── TapOrHold (dictation != bare Option)
│   │   ├── Clean Option tap (< 280 ms) -> toggle pinned open/closed
│   │   ├── Hold Option alone (>= 280 ms) -> temporary preview; release hides
│   │   └── Menu bar › Today’s Steps -> toggle pinned open/closed
│   ├── TapOnly (dictation == bare Option in Hold mode)
│   │   ├── Clean Option tap (< 280 ms) -> toggle pinned open/closed
│   │   │   └── double_tap_lock on -> defer 300 ms; 2nd press cancels (zero flash)
│   │   ├── Hold Option alone (>= 280 ms) -> preview while held, beside the dictation
│   │   │   ├── Clicking a step or Pin cancels that dictation (nothing pastes)
│   │   │   └── Start tone waits until hold registers so Option taps stay silent
│   │   └── Menu bar › Today’s Steps -> toggle pinned open/closed
│   └── MenuBarOnly (dictation == bare Option in Toggle mode)
│       ├── Option tap/hold -> reserved for Toggle dictation (no overlay action)
│       └── Menu bar › Today’s Steps -> toggle pinned open/closed
├── States (display under pointer, upper-right, non-activating panel)
│   ├── Committed plan -> focus, progress bar, checkable steps (in-place order),
│   │                     Open planner button; All done banner when complete
│   ├── Uncommitted draft -> warning Draft · not committed pill + Finish planning →
│   │                        (read-only step preview; header Open planner hidden)
│   └── Empty (no plan yet) -> Plan today button (header Open planner hidden)
├── Interaction & focus
│   ├── Opening pinned never calls request_keys() -> active app keeps typing focus
│   ├── Clicking a step toggles & saves immediately (and cancels any Option capture)
│   │   └── When pinned -> selects row & requests keys for ↑↓ / Space / Enter / Esc
│   ├── Losing overlay window focus -> clears selection & keyboard hint
│   └── Intentional dictation (>= 300 ms or control/API) unpins; while Option
│       is not held it hides, as do lock / sleep / opening Planner / disabling Lab
```

In `TapOnly` mode (bare Option in Hold mode with Labs › Planner on), the
dictation or second-dictation shortcut bound to bare Option defers its start
tone until the 300 ms intentional-hold threshold so short Option taps that
toggle Today's Steps stay silent without a 40 ms blip; non-Option shortcuts,
raw dictation, Caps Lock, and Toggle mode keep their normal 40 ms provisional
start tone. Short-tap provisional captures (< 280 ms) are cancelled rather than
finished so stray background audio is never transcribed, and provisional
presses never unpin an already-pinned Today's Steps card — so tapping Option
while pinned closes it cleanly without flickering back open. Once dictation
passes the 300 ms intentional-hold threshold (or starts via control/API),
Today's Steps unpins, and stays up only while Option is still held.

Drafts save after edits and immediately on navigation, step toggles, or
dismissal, using Hex's crash-safe owner-only writer (0600 on macOS). A failed
load leaves the original untouched and refuses replacement until restart. The
file has an 8 MB limit; individual submitted answers/items have an 8 KB limit
and a daily checklist has at most 100 items. There is no network service or AI
dependency. Emergency dismissal still works if storage fails.

Sources: [ledger and validation](../../src/rune_ui/planner/model.rs),
[flow and saving](../../src/rune_ui/planner/mod.rs),
[slide chrome and helpers](../../src/rune_ui/planner/view/mod.rs),
[slide layouts](../../src/rune_ui/planner/view/slides.rs),
[Today’s Steps overlay and gesture detectors](../../src/rune_ui/planner/peek.rs),
[native schedule and presentation](../../src/rune_ui/planner/macos.rs),
[macOS runtime pump](../../src/macos_rune.rs),
[menu-bar item](../../src/macos_rune/status_item.rs),
[recognition tap & tone deferral](../../src/recognition.rs),
[hotkey tone-deferral predicate](../../src/recognition/hotkeys.rs),
[login item adapter](../../src/desktop/login_item.rs).

Checks: [model tests](../../src/rune_ui/planner/model/tests.rs) cover input
minimums, date formatting, Eastern 6 AM boundaries, weekly timing, carry-over
triage seeding, step toggling, commit gates, private persistence,
restart/bypass, and damaged files.
[Planner UI tests](../../src/rune_ui/planner/tests.rs) drive both flows,
yesterday check-in and carry-over triage, Yes/Not yet step sync, safe Edit plan
rollback, `OptionPeekMode` resolution (`TapOrHold`, `TapOnly`, `MenuBarOnly`),
adaptive Yesterday footer hints, multi-field Enter focus movement, editing
arrows, pasted input, failed saves, manual reopen to DayView, two-stage Escape,
header Y consistency and 1024×768 fit assertions, native Eastern date
calculation, and offscreen renders of all nine slides in both appearances.
[Peek tests](../../src/rune_ui/planner/peek.rs) cover visibility rules,
checkable/pinnable/keyboard-controllable pinned steps, window-blur highlight
clearing (`on_focus`), read-only drafts, empty/mode-adapted footer and button
copy, `OptionTapDetector` (clean tap, hold, chords, keys, clicks, double-tap
300 ms deferral), `HoldPreviewGate::tick` + `OptionTapDetector` consistency at
100, 200, 279, 280, and 400 ms from both closed and pinned, real-sequence
tap-to-close / hold / double-tap-lock behavior against `DictationHotkey`
(`option_tap_closes_pinned_overlay_even_when_option_starts_provisional_dictation`),
100-task scrolling, and light/dark renders of base, pinned, keyboard-selected,
all-done, draft, and empty states.
[Hotkey tests](../../src/recognition/hotkeys.rs) cover cancelling active
capture on overlay click (`cancelling_active_capture_prevents_option_release_from_transcribing`)
and narrowing start-tone deferral exclusively to bare-Option Hold dictation
(`planner_only_defers_start_tone_for_bare_option_hold_dictation`).
`planner_is_searchable_and_its_toggle_reveals_its_pane_only_on_macos` in
[pane tests](../../src/rune_ui/tests/panes.rs) exercises the Labs card list and
sidebar visibility.

Proof limits: these offscreen and unit checks do not prove real microphone
transcription/paste into another app after clicking a step in Today’s Steps,
real physical Option taps/double-taps, login after a real reboot, VoiceOver
behavior, multiple physical monitors, or ordering of real system dialogs.
Native panels use floating levels rather than kiosk mode; OS shortcuts remain
usable by design.
