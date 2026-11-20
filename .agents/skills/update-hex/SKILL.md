---
name: update-hex
description: Update this personal Hex fork from anomalyco/hex dev, test the merge, and install a verified local macOS build. Use for Hex updates and the Monday update routine in slippy-hex.
---

# Update Hex

Work in `/Users/aakashdwa/Documents/ChatGPT/slippy-hex` on this Mac.
Read the repository's `AGENTS.md`, `CONTRIBUTING.md`, and
[personal fork guide](../../../docs/personal-fork.md). The user's fork
instructions take precedence over upstream's release and dependency-bump
instructions: keep upstream `dev` as the base, retain the small personal
overlay, and use `HEX_LOCAL_BUILD=1`. Labs stays available in the UI with
individual opt-ins; preserve saved choices. GitHub Actions is disabled in
this fork. The Codex routine owns updates, not a release feed.

## Merge and verify

1. Inspect the worktree, branch, remotes, and open fork PRs. Require `origin`
   to be `balaji-dwa/hex` and `upstream` to be `anomalyco/hex`. Leave user
   edits intact; report a dirty worktree or an existing unresolved update PR
   instead of resetting, stashing, or starting another update. Use an
   exclusive `mkdir .git/hex-update.lock` to prevent overlapping updates;
   remove only your own lock when done. Report an existing lock.
2. Fetch `origin dev` and `upstream dev`. Record both SHAs. If upstream is
   already an ancestor of `origin/dev`, the source is current; check whether
   the installed local build needs catching up, without rebuilding an
   already verified identical commit.
3. For a new upstream head, create a unique `codex/upstream-sync-<timestamp>`
   branch from `origin/dev` in a temporary worktree. Merge the exact fetched
   upstream SHA normally. Keep all fork changes; avoid dependency upgrades,
   new infrastructure, or changing app behavior to make the update easier.
4. On a clean merge run `HEX_LOCAL_BUILD=1 scripts/check`,
   `scripts/test-app-identity.sh`, and `git diff --check`. All must pass before
   advancing `dev`. Push the candidate and create a PR in **balaji-dwa/hex**,
   with the tested SHAs, results, and relevant upstream changes. Advance
   `dev` with a normal push of the exact tested candidate SHA to
   `refs/heads/dev`; GitHub will mark the candidate PR merged. This preserves
   upstream ancestry and rejects a concurrent divergent push. If rejected,
   leave the PR for review; do not ask GitHub to compute an untested merge.
5. On conflict, list conflicting paths, abort the disposable merge, and push
   a review branch pointing to the fetched **upstream head**. Open a fork PR
   explaining how to merge `origin/dev` into that branch and resolve it while
   keeping the personal files. On test failure, push the merged candidate
   and open a fork PR with the failing commands and logs. Leave `dev` and the
   running app unchanged. Do not commit conflict markers or force-push.

## Install and confirm

After a verified merge, fast-forward the clean primary checkout's `dev` from
`origin/dev`. Install using `HEX_TEAM_ID=6823DZMTQD scripts/install-personal-macos.sh`.
This is authorized by the user's standing request to keep their personal
local app current. An active recording may refuse the installer's ordinary
quit; report that and leave it running. Keep signing credentials in the
keychain. If signing, permission, build, or launch fails, retain the old app
and report the concrete blocker; preserve platform security checks.

Verify the process executable is `/Applications/Hex Local.app/Contents/MacOS/hex`,
the app reports local-build mode with updates off, and settings/history are
still available. Keep exactly one installation: `/Applications/Hex Local.app`.
The installer verifies the running executable before unregistering and moving
its previous app and packaged development copy to macOS Trash. Retain data
backups, settings, history, models, and source/build caches.

Before finishing any update or development session, including a no-update
run, check for other Hex app bundles in `/Applications`, `~/Applications`,
the project's `target/app`, and the user's Hex/Codex output and app-backup
folders (Spotlight can locate additional copies). Confirm each candidate's
bundle ID belongs to Hex, exclude the current installation and Trash, and
unregister and move the other app bundles to Trash through macOS's native
file API. Stop any development app through its ordinary quit first; respect
active recordings. Re-register the surviving local app and verify that
quitting and reopening by bundle ID starts that exact path. This cleanup is
authorized by the user's standing single-installation preference. Never
empty Trash or delete the accompanying user data.

Record the installed commit, data-backup path, tests, and retired app copies.
A failed install or cleanup is partial completion; report its blocker rather
than claiming only one installation remains.

Remove only the disposable worktree and your lock, keeping any failure PR
branch for review. Report new changes, a completed install, or a blocker;
an unchanged already-installed commit needs only a concise no-update result.
