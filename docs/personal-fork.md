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

`Personal upstream sync` runs weekly on Monday at 09:53 UTC, and can be
started manually:

```sh
gh workflow run personal-upstream.yml --repo balaji-dwa/hex --ref dev
```

The workflow merges upstream `dev` into a disposable checkout, then runs
the fork's Git integration checks, upstream's app-identity checks, and
`scripts/check` (formatting, code shape, Nix pins, clippy, and macOS Rust tests).
It uses a standard GitHub macOS runner, without signing or release credentials.
Only a successful merge and test job can advance `dev`, using a normal Git
push. A concurrent change to `dev` rejects that push safely.

On conflict, failed checks, or a rejected push, it pushes a unique review
branch and opens a PR. A conflict branch contains the actual upstream head;
no unresolved markers are committed. Resolve it locally:

```sh
git fetch origin
gh pr checkout <number>
git merge origin/dev
# Resolve conflicts, keeping the fork's local install/sync safeguards.
git add <resolved-files>
git commit
python3 scripts/test-personal-fork.py
scripts/check
git push
```

Review the PR, then merge it in the fork. An open sync PR pauses further
sync attempts; closing it without merging permits a fresh attempt. Never
force-push `dev`. Once merged, update your checkout with
`git switch dev && git pull --ff-only origin dev` and reinstall explicitly.
The weekly workflow changes source only, never your laptop's installed app.

Fork setup disables inherited upstream workflows in GitHub's workflow
settings (their files stay unchanged to reduce conflicts). Keep release,
site deployment, SDK publishing, and upstream agent workflows disabled.
Audit newly introduced workflows after upstream merges. Enable only
`personal-upstream.yml`; enable GitHub Actions' “Allow GitHub Actions to
create and approve pull requests.” Scheduled workflows in inactive public
repositories can be disabled by GitHub after 60 days; re-enable this workflow
if that happens. No PAT or signing secret belongs in Actions.

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
validates the signed bundle through `scripts/validate-app.sh`, and renders a
settings scene using a temporary data root. It asks the existing app to quit
through Hex's control command (without `--force`); active recording can refuse.
It backs up the full `hex` data folder and `~/.config/hex` under
`~/Library/Application Support/Hex Local Backups/` before launching.

It installs to `/Applications/Hex Local-<commit>.app`, refuses to overwrite
any existing destination, verifies its signature again, registers it with
Launch Services, and launches it. `/Applications/Hex.app` stays intact. The
new copy deliberately keeps upstream's bundle ID and data paths so settings,
history, models, and config persist; run one copy at a time. A new signing
identity may require fresh macOS permissions. Launch at login is controlled
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

After verifying dictation, settings, history, and login behavior, older
bundles are deletion candidates: move the **app bundles only** to Trash in
Finder. Keep the current `Hex Local-<commit>.app`, the `hex` data folder,
`~/.config/hex`, and backups. To roll back, quit the local app and open the
old bundle; if data migration requires restoring a snapshot, preserve the
current data separately first. Restoring an older snapshot loses changes
since that snapshot. Neither the installer nor the sync workflow deletes
old apps or data.

## Propose a feature upstream

The maintainers [accept issues with patches, not outside PRs](../CONTRIBUTING.md).
Start a small proposal branch directly from current upstream `dev`, apply
only the feature commits (omit this fork's workflow and installer), then:

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
