---
name: release-hex
description: Sync this Hex fork with upstream anomalyco/hex dev (if new commits exist), verify, build and notarize Slippy Hex on macOS, publish to the team release feed, and handle rollback.
---

# Release Slippy Hex to the Team

Work in `/Users/aakashdwa/Documents/ChatGPT/slippy-hex` on this Apple Silicon Mac.
Never hardcode private hostnames, bucket names, account IDs, or credentials in git.
Runtime release config lives outside the repo in `~/.config/hex/team-release.env`;
secrets live in macOS Keychain (`hex-notary` for Apple notarization), `~/.aws/credentials`
(profile `hex-r2`), and 1Password (`slippy-production` vault).

## 1. Sync with upstream (when running scheduled or syncing)

1. Confirm `origin` is `balaji-dwa/hex` and `upstream` is `anomalyco/hex`.
   Leave user edits intact; if the primary checkout is dirty or an unresolved
   upstream-sync PR is open, stop and report the blocker instead of resetting or stashing.
2. Acquire an exclusive lock with `mkdir .git/hex-release.lock` (remove only your own lock
   on exit; report an existing lock and stop).
3. Fetch `origin dev` and `upstream dev`.
4. If `upstream/dev` is not yet an ancestor of `origin/dev`:
   - Create a temporary worktree on a new branch `codex/upstream-sync-<timestamp>` from `origin/dev`.
   - Merge the fetched `upstream/dev` SHA cleanly without force-pushing or dropping fork changes.
   - On merge conflict: abort the merge, push a review branch pointing to the fetched `upstream/dev`,
     open a fork PR listing the conflicting files, leave `dev` and the live team feed untouched,
     and report the blocker.
   - Run `scripts/check`, `scripts/test-app-identity.sh`, `python3 -m unittest discover -s scripts/team-release -p 'test_*.py'`,
     `node --test packaging/team-release-worker/worker.test.mjs`, and `git diff --check`.
   - On test failure: push the merged candidate, open a fork PR with the failing command output,
     leave `dev` and the live team feed untouched, and report the blocker.
   - On pass: push the tested candidate, open a fork PR recording the SHAs and checks, and
     fast-forward `origin/dev` to the exact tested commit, then fast-forward local `dev`.

## 2. Decide whether a new team release is needed

Compare `git rev-parse HEAD` on clean `dev` against the latest published commit in
`<HEX_RELEASE_ORIGIN>/hex/files/dev-notes.json` (first entry of `builds[0].commit`).
The release host is protected by Cloudflare Zero Trust; source `~/.config/hex/team-release.env`
and pass `-H "CF-Access-Client-Id: $HEX_RELEASE_CF_ACCESS_CLIENT_ID"` and
`-H "CF-Access-Client-Secret: $HEX_RELEASE_CF_ACCESS_CLIENT_SECRET"` when fetching
`dev-notes.json`.
- If `HEAD` matches the already-published commit (and this is a scheduled check), report
  that the team feed is already up to date and exit without rebuilding.
- If `HEAD` is newer (either from a fresh upstream merge or your own new commits on `dev`),
  proceed to publish.

## 3. Build, sign, notarize, and publish

Run from a clean `dev` checkout:

```sh
scripts/team-release/release.sh
```

What this script does automatically:
- Loads `~/.config/hex/team-release.env` (`HEX_TEAM_ID`, `HEX_APP_NAME="Slippy Hex"`,
  `HEX_NOTARY_PROFILE`, `HEX_RELEASE_ORIGIN`, `HEX_R2_BUCKET`, `HEX_R2_ENDPOINT`,
  `HEX_R2_AWS_PROFILE`).
- Runs `scripts/test-app-identity.sh` and `scripts/check updater:: dev_build:: labs:: app_paths::`.
- Builds `Slippy Hex.app` with `HEX_BUILD_CHANNEL=dev`, `HEX_UPDATE_TEAM_ID=$HEX_TEAM_ID`,
  and compile-time release origin/notes URLs.
- Validates the bundle with `scripts/validate-app.sh`.
- Notarizes and staples `Slippy Hex.app` and `Hex.dmg` via `scripts/notarize-app.sh`,
  packs the stapled `Slippy Hex.app` into `Hex.zip`, and verifies `codesign -R` and `spctl`.
- Uploads immutable `hex/files/<sha256>/Hex.zip` and `Hex.dmg` to R2, verifies remote byte
  sizes over HTTPS, uploads `dev-notes.json`, and updates `hex/desktop/dev.json` last.
- Verifies live `GET /hex/desktop/dev.json` and `HEAD /download` redirect.
- Archives each published manifest under `~/.local/share/hex-team-releases/history/<version>.json`.

## 4. Dry run (local check without publishing)

```sh
scripts/team-release/release.sh --dry-run --skip-notarize --allow-dirty
```

(`--skip-notarize` is rejected unless `--dry-run` is set so unnotarized builds can never be published.)

## 5. Roll back a bad release

Hex's updater follows whatever release is current in `dev.json` without comparing versions.
To roll every teammate back to a previous release in seconds:

```sh
scripts/team-release/release.sh --rollback ~/.local/share/hex-team-releases/history/<version>.json
```

This verifies the previous `Hex.zip` and `Hex.dmg` are still present on the release host,
bumps `sequence`, and repoints `dev.json` and `/download` back to that release.

## 6. First-time teammate install

Send teammates the single permanent download URL: `<HEX_RELEASE_ORIGIN>/download`.
They open `Hex.dmg`, drag `Slippy Hex.app` into `/Applications`, and launch it.
Subsequent updates download automatically in the background and install on Restart to update.
