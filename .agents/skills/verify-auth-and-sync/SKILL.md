---
name: verify-auth-and-sync
description: Model-check Hex Team sign-in refresh and the Slippy restart cursor. Use when changing auth retry, member teardown, cursor seeding, or catch-up.
---

# Verify Team sign-in and Slippy sync

1. Compare each changed transition with the source listed below. Update the model when the behavior changes.
2. Run `./.agents/skills/verify-auth-and-sync/scripts/check.sh` from the repository root. It uses TLC 1.7.4 and Temurin 21 from `~/.cache/slippy-tlc`. Set `TLA2TOOLS_JAR` and `JAVA` for other install paths.
3. Read the results with pstack's `/statechart` skill. A passing run reports zero states left on both queues and no errors. Trace any violation back to its Rust transition before changing the property.

## Coverage

- `TeamAuthRefresh` follows `Client::refresh_if_needed` and `AuthState::reauth` in `src/team_auth.rs`, `Card::complete`, `Card::poll`, and `Card::needs_reauth` in `src/rune_ui/labs/team_auth.rs`, and reporting and worker cleanup in `src/slippy/sync.rs`. It checks that an outage or cancellation preserves the member, the cursor and cache, and a retry. Terminal auth errors remove the member and stop retries. Fair worker shutdown eventually removes the files. The model starts with one signed-in member and saved state and abstracts the 15-second backoff as a scheduled retry.
- `SlippySyncCursor` follows `Cursor::load`, `sync`, `delta`, and `settle` in `src/slippy/sync.rs`. It checks first seed, restart from every saved sequence including zero, unread and notification delivery, and eventual catch-up. It enumerates task sequences 1–5, all eligible finished subsets, every two-row snapshot selection, and up to three detail delays. It folds ordered delta pages into per-task steps. Like `listHexThreads`, the model treats zero as a snapshot and positive values as ascending delta cursors. Sequence 1 starts queued. The model freezes server history during catch-up and assumes working requests, disk writes, and notification delivery. Rust tests cover the real 100-row page limit.

Coverage was checked against the uncommitted implementation based on `29dbd5b07066a4ac9cd112a480a0021fb1d0f2df`. Record the implementation commit after confirming the models still match it.
