# Personal Hex fork

This fork uses [anomalyco/hex's `dev`](https://github.com/anomalyco/hex/tree/dev)
as its canonical base. `origin` is `balaji-dwa/hex`; `upstream` is
`anomalyco/hex`. The fork's default branch is `dev`. There is no separate
product, update service, or release pipeline. The setup changes no application
code: upstream already provides Labs and `HEX_LOCAL_BUILD=1`.

## Add a feature

Start from the fork's current `dev`:

```sh
git switch dev
git pull --ff-only origin dev
git switch -c codex/my-lab
```

Read `AGENTS.md`, `CONTRIBUTING.md`, and `docs/features/labs.md`. Keep the
implementation in one small module beside the capability it changes, with a
single gate at the point work starts. Reuse the existing registry; do not add
a parallel Labs framework or master switch. The places a new lab reaches are:

- `src/app_settings/labs.rs`: its persisted boolean, explicitly false by default.
- `src/labs/feature.rs`, `catalog.rs`, `card.rs`, `description.rs`, `group.rs`:
  registry entry, stable name, audience, OS availability, text, and group.
- `src/labs.rs` and `src/labs/gates.rs`: setting read/write, runtime state, gate.
- `src/control/settings/`: the corresponding `hex settings` key.
- Its worker/caller and `src/rune_ui/labs.rs` if the existing generic card is
  insufficient; translated strings in `src/desktop/i18n/`.
- A focused off/on test, including the off path doing no work, and the
  affected feature map. Update UI goldens on Linux if the drawn UI changes.

Prefer the `Dev` audience for personal experiments. A local build includes
Dev labs, but does **not** enable their switches. You can use Settings › Labs
as usual to opt in; your existing settings remain in place. For an Everyone
lab, regenerate its website data with
`HEX_UPDATE_LABS_JSON=1 scripts/check labs::web` and commit the result.
Run `scripts/check`, open a PR **in your fork** to `dev`, then merge it. Avoid
dependency bumps and unrelated refactors when adding a lab.

## Update the fork

The **Update personal Hex** Codex routine runs in this project every Monday
at **12:00 noon, America/New_York**, on this Mac. It invokes the project skill
[update-hex](../.agents/skills/update-hex/SKILL.md); you can also invoke
`$update-hex` manually in a project chat. The Mac and Codex need to be
available for a local run. Change its schedule or pause it in Codex's
automations UI.

The skill merges upstream `dev` into a disposable worktree and runs upstream's
`scripts/check` (formatting, code shape, Nix pins, clippy, and macOS Rust tests),
app-identity checks, and `git diff --check`. A successful candidate goes
through a PR in your fork, then advances `dev` to the exact tested candidate
with an ordinary fast-forward push. A concurrent divergent push is rejected.
It then installs a
verified local build using the command below. It leaves user edits intact
and stops if there is an existing unresolved update PR or update lock.

Conflicts and failed checks get a review PR, leaving `dev` and the installed
app intact. A conflict branch contains the actual upstream head, without
committed conflict markers. Resolve it locally:

```sh
git fetch origin
gh pr checkout <number>
git merge origin/dev
# Resolve conflicts, keeping the fork's local install safeguards.
git add <resolved-files>
git commit
scripts/check
git push
```

Review the PR, then merge it in the fork. An open update PR pauses further
attempts; closing it without merging permits a fresh attempt. Never
force-push `dev`. Once merged, update your checkout with
`git switch dev && git pull --ff-only origin dev` and reinstall explicitly.
The routine reports a source update with a failed installation as partial
completion, including the blocker and PR or installed commit.

**GitHub Actions is disabled for this fork.** The personal sync workflow was
removed; inherited workflow files remain unchanged to reduce conflicts.
The Codex routine uses your existing local GitHub login and keychain signing
identity, with no CI secrets, update feed, or server.

## Build and install locally on macOS

Requires Apple silicon, full Xcode 26 or later with its Metal toolchain,
Rust stable with rustfmt/clippy, CMake, jq, Python 3, and cargo-nextest.
For a writable Homebrew installation:

```sh
brew install rustup cmake jq cargo-nextest
rustup default stable
rustup component add rustfmt clippy
xcrun --sdk macosx metal --version || xcodebuild -downloadComponent MetalToolchain
```

If Homebrew belongs to a different macOS account, do not recursively change
its ownership. User-wide `uv tool install cmake` and nextest's official
[prebuilt binary](https://nexte.st/docs/installation/pre-built-binaries/) work
too; keep their executable directories on your PATH.

The normal upstream bundle script requires a **Developer ID Application**
certificate and private key in your login keychain. Apple Distribution is
for a different distribution path and is rejected by the script. This is an
upstream packaging prerequisite, not a macOS requirement for all local code.
Use Xcode › Settings › Apple Accounts › your team › Manage Certificates, or
Apple's certificate portal. Never export signing keys into the repository or
CI. Inspect available identities with `security find-identity -v -p codesigning`.

From a clean, committed checkout:

```sh
HEX_TEAM_ID=6823DZMTQD scripts/install-personal-macos.sh
```

This runs relevant Labs/local-build/updater tests, builds through upstream's
`scripts/build-app.sh` with locked dependencies and `HEX_LOCAL_BUILD=1`,
validates the signed bundle through `scripts/validate-app.sh`, and checks its
version and empty History CLI using a temporary data root. It asks the existing app to quit
through Hex's control command (without `--force`); active recording can refuse.
It backs up the full `hex` data folder and `~/.config/hex` under
`~/Library/Application Support/Hex Local Backups/` before launching.

It installs to the stable path `/Applications/Hex Local.app`, verifies the
staged signature again, and temporarily saves the existing local bundle in
the data backup before replacing it. A failed move restores that previous
bundle. It registers and launches the app, then verifies the running executable
is exactly the stable installed path. Only after that passes does it unregister
and move the previous app and packaged development copy to macOS Trash. The
new copy deliberately keeps upstream's bundle ID and data paths so settings,
history, models, and config persist; run one copy at a time. A new signing
identity may require fresh macOS permissions. Future installs reuse the same
path, bundle ID, and Developer ID team; the commit is only a build label,
not a new app identity. Launch at login is controlled
in the UI, through macOS; verify it points to your chosen local copy.

Open Settings and verify the footer says **Local build · updates off** and
the commit matches `git rev-parse --short=12 HEAD`. Labs remains available in
the UI. Check the running path with:

```sh
ps -axo pid,args | rg '/Applications/Hex[^/]*/Contents/MacOS/hex'
```

The command does not notarize or distribute the app. It never removes
quarantine, disables Gatekeeper, changes trust rules, or bypasses signature
checks. If macOS refuses launch, leave its protections in place and use
upstream's `scripts/notarize-app.sh` with your own notary credentials before
retrying. `HEX_TEST_BUILD=1 scripts/build-app.sh` is upstream's ad-hoc signed
alternative, but its separate `hex-test` data folder is unsuitable for this
in-place continuation of your existing settings/history.

Keep one installation: `/Applications/Hex Local.app`. The update routine also
checks installed, build, output, and app-backup folders for extra Hex bundles,
unregisters them, and moves the **app bundles only** to Trash, including
temporary development apps once their verification is finished. It verifies
quit/reopen resolves to the surviving local app. This policy also applies when
upstream has no new update. Data backups stay intact.

Keep the `hex` data folder, `~/.config/hex`, and data backups. For rollback,
quit the app, restore the desired previous app from Trash, and replace the
current local app intentionally so only one copy remains installed. If data
migration requires restoring a snapshot, preserve the current data separately
first. Restoring an older snapshot loses changes since that snapshot. Neither
the installer nor the routine empties Trash or deletes user data.

## Propose a feature upstream

The maintainers [accept issues with patches, not outside PRs](../CONTRIBUTING.md).
Start a small proposal branch directly from current upstream `dev`, apply
only the feature commits (omit this fork's skill and installer), then:

```sh
git fetch upstream dev
git diff --check upstream/dev...HEAD
git diff --binary upstream/dev...HEAD > /tmp/hex-feature.patch
```

Open [an upstream issue](https://github.com/anomalyco/hex/issues/new/choose).
Describe the problem, before/after behavior, OS, Labs opt-in/default, tests
actually run, and limitations. Include the small diff in a fenced `diff`
block or link a published patch. Redact paths, transcripts, logs, credentials,
and personal settings. Submit the issue intentionally; automation never
posts to maintainers or publishes fork builds.
