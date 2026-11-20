# Planner

Planner is a macOS-only, Everyone Lab, off by default (`labs.planner`).
Its ledger is local: `planner.json` in Hex's Application Support folder,
separate from dictation History and its retention settings. The initial
planner slides use English; the Lab card follows the interface language.

```ts
Labs › Planner on                                      // planner.opt-in
├── Add Planner to the sidebar -> Open planner anytime
├── Register Hex with macOS Launch at login
│   └── macOS requires approval / registration fails -> explain in Planner
└── Check at launch and once a second while Hex runs     // planner.schedule
    ├── Display asleep, Mac asleep, or session locked -> wait until return
    ├── Completed for this planning day -> stay hidden
    └── Due -> full-screen Rune panel on the pointer's display
        ├── Other displays -> ordinary native backdrops; follow display changes
        ├── Emergency bypass / Escape -> save draft, hide for five minutes
        │   └── Deadline expires -> show again, unless completed or away
        └── System modal/security UI, menu bar, lock, Force Quit -> remain available
```

The day starts at **6 AM America/New_York**, including daylight-saving time,
independent of the Mac's current time zone. A saved five-minute bypass
survives app restarts. Hex must be running for the reminder to appear;
quitting remains available. Turning the Lab off stops presentation and keeps
the ledger. Launch at login is the existing OS-owned setting and remains on
when Planner is turned off.

```ts
Daily planning                                         // planner.daily
├── Big picture -> goal, weekly focus, success criteria, remaining work,
│                 completed deliverables and checked steps
├── Yesterday -> show the deliverable and criteria -> Yes / Not yet
├── Yesterday’s steps -> check off the list; no add row or text answer
│   └── Missed days -> label the last plan with its actual date
├── Today -> choose one problem or piece
├── Breakdown -> today's problem above editable steps (add/remove only)
└── Commit -> concrete deliverable AND explicit completion criteria
    ├── Invalid answer or checklist -> stay on the slide with an explanation
    ├── Disk save fails -> keep the draft; never mark completion
    └── Saved -> hide until the next planning day

Weekly review                                         // planner.weekly
├── First enable -> overarching goal + remaining work -> weekly focus + success
└── Seven calendar days since the last completed review
    ├── Progress -> previous focus, saved deliverables, completed steps
    ├── Reflect -> progress, obstacles, what should change
    ├── Goal -> keep or revise the goal and remaining work
    └── Focus -> next week's focus and success criteria -> save -> daily flow
```

Open-ended answers require at least **five whitespace-separated words that
contain a letter or number**. Checklist items require at least **three
trimmed Unicode characters**, including when edited. Final completion also
checks the entire plan, not just the visible slide. The previous deliverable
needs an explicit Yes / Not yet when there is a previous plan. A checklist
entry waiting for Add must be added or cleared before leaving that slide.

Each open-ended question has its own slide with a large text area. Word-count
labels and slide numbers are hidden; the minimum is explained only on an
invalid submission. The header has icon-only previous/next arrows before Daily / Weekly; each question is its own title.

The keyboard works without global interception: Left/Right navigate outside
an editor; inside an editor they move the caret. Enter advances/submits (Shift+Enter adds a line);
Enter in Add an item adds that item. Tab moves through named controls. Hex
dictation can paste into the normal Rune editors. The small, subdued bypass
stays in the footer's bottom-right, outside the scrolling content. Reopening
a completed plan starts at its breakdown; closing it does not schedule another
reminder that day. Escape is the same five-minute bypass while planning is due.
On the first day, with no previous plan, the two yesterday pages are skipped.

Checkboxes appear only while reviewing the previous plan. The first visit to
today’s breakdown copies the steps still unchecked after that review. Later
visits and restarts preserve additions/removals instead of seeding again.

Drafts save after edits and immediately on navigation or dismissal, using
Hex's crash-safe owner-only writer (0600 on macOS). A failed load leaves the
original untouched and refuses replacement until restart. The file has an
8 MB limit; individual submitted answers/items have an 8 KB limit and a
daily checklist has at most 100 items. There is no network service or AI
dependency. Emergency dismissal still works if storage fails.

Sources: [ledger and validation](../../src/rune_ui/planner/model.rs),
[flow and saving](../../src/rune_ui/planner/mod.rs),
[slides](../../src/rune_ui/planner/view.rs),
[native schedule and presentation](../../src/rune_ui/planner/macos.rs),
[macOS runtime pump](../../src/macos_rune.rs),
[login item adapter](../../src/desktop/login_item.rs).

Checks: [model tests](../../src/rune_ui/planner/model/tests.rs) cover input
minimums, date boundaries, weekly timing, commit gates, private persistence,
restart/bypass, and damaged files.
[Planner UI tests](../../src/rune_ui/planner/tests.rs) drive both flows,
editing arrows, pasted input, Enter, failed saves, manual reopen, Escape,
the footer position, native Eastern date calculation, and offscreen renders
of all thirteen slides in both appearances. The ordinary Rune suite also covers
the Planner sidebar fixture, Lab card and translations, and registry.

These checks do not establish real microphone transcription/paste, login
after a real reboot, VoiceOver behavior, multiple physical monitors, or the
ordering of real system dialogs. Native panels use floating levels rather
than kiosk mode or keyboard event taps; OS shortcuts remain usable by design.

Executed November 20, 2026: the macOS `scripts/check` run passed formatting,
code shape, Nix pins, strict workspace Clippy, and 3,380 tests (34 opt-in
checks skipped). After the final copy changes, all 20 Planner tests and
strict all-target Clippy passed again. Linux's Rune UI suite passed 797
checks (three skipped), the final Planner suite passed 19, and all nine
existing golden-test groups passed against their files. The 28 new Planner
renders and two updated Labs renders were inspected in both appearances.
These are offscreen/test results, subject to the native proof limits above.
