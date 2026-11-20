#!/bin/bash
# Build a committed local app; keep the previous local bundle for rollback.
set -euo pipefail
umask 077
cd "$(dirname "$0")/.."
[[ $(uname -s) == Darwin && $(uname -m) == arm64 ]] || {
  echo 'Requires an Apple silicon Mac.' >&2; exit 1;
}
[[ -z $(git status --porcelain) ]] || {
  echo 'Commit your changes before building a reproducible local app.' >&2; exit 1;
}
: "${HEX_TEAM_ID:?Set HEX_TEAM_ID to your Developer ID signing team}"
. scripts/macos-signing.sh
hex_codesign_identity "$HEX_TEAM_ID" >/dev/null
commit=$(git rev-parse --short=12 HEAD)
destination='/Applications/Hex Local.app'
if [[ -e "$destination" || -L "$destination" ]]; then
  [[ ! -L "$destination" ]]
  codesign --verify --deep --strict "$destination"
  signature=$(codesign -dv --verbose=4 "$destination" 2>&1)
  [[ "$signature" == *"TeamIdentifier=$HEX_TEAM_ID"* ]] || {
    echo 'Existing Hex Local.app belongs to another signing team; leaving it intact.' >&2; exit 1;
  }
fi
# These select distribution or isolated test builds instead of our local app.
unset HEX_TEST_BUILD HEX_RELEASE_UNSIGNED HEX_BUILD_CHANNEL HEX_VERSION HEX_BUILD_NUMBER
unset HEX_APPLICATION_SUPPORT_DIR CARGO_TARGET_DIR
export HEX_LOCAL_BUILD=1 HEX_COMMIT="$commit"
scripts/test-app-identity.sh
scripts/check labs:: dev_build:: updater:: app_paths::
bundle=$(scripts/build-app.sh)
version=$(hex_version "$PWD")
scripts/validate-app.sh "$bundle" Hex.app "$version" "$(hex_build_number "$version")"
temporary=$(mktemp -d)
trap 'rm -rf "$temporary"' EXIT
binary="$bundle/Contents/MacOS/hex"
[[ $("$binary" --version) == "hex $version" ]]
HEX_APPLICATION_SUPPORT_DIR="$temporary/data" "$binary" history --json > "$temporary/history.json"
python3 -c 'import json,sys; assert json.load(open(sys.argv[1]))["entries"] == []' "$temporary/history.json"
staging=$(mktemp -d /Applications/.Hex-Local-install.XXXXXX)
trap 'rm -rf "$temporary" "$staging"' EXIT
ditto "$bundle" "$staging/Hex.app"
codesign --verify --deep --strict "$staging/Hex.app"
# A refused quit (for example, an active recording) aborts installation.
if "$binary" status --json > "$temporary/status.json" 2>/dev/null; then
  pid=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["pid"])' "$temporary/status.json")
  "$binary" quit
  for ((attempt=0; attempt<30; attempt++)); do
    kill -0 "$pid" 2>/dev/null || break
    sleep 1
  done
  if kill -0 "$pid" 2>/dev/null; then
    echo 'Hex has not stopped; leaving all installed apps intact.' >&2; exit 1
  fi
fi
# Snapshot data while Hex is stopped, before a newer build can migrate it.
backup="$HOME/Library/Application Support/Hex Local Backups/$(date +%Y%m%d-%H%M%S)-$commit"
mkdir -p "$backup"
if [[ -d "$HOME/Library/Application Support/hex" ]]; then
  ditto "$HOME/Library/Application Support/hex" "$backup/data"
fi
if [[ -d "$HOME/.config/hex" ]]; then
  ditto "$HOME/.config/hex" "$backup/config"
fi
if [[ -e "$destination" ]]; then
  mv "$destination" "$backup/Hex Local.app"
fi
if ! mv "$staging/Hex.app" "$destination"; then
  [[ ! -e "$backup/Hex Local.app" ]] || mv "$backup/Hex Local.app" "$destination"
  exit 1
fi
'/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister' -f "$destination"
open "$destination"
printf 'Installed: %s\nData backup: %s\n' "$destination" "$backup"
echo 'Older Hex bundles can be moved to Trash after verifying this copy in Settings.'
