#!/usr/bin/env bash
# Build, sign, notarize, package, and publish (or roll back) a team macOS release
# compatible with Hex's native schema-1 in-app updater.
set -euo pipefail
umask 077

root="$(CDPATH='' cd -- "$(dirname -- "$0")/../.." && pwd)"
manifest_py="$root/scripts/team-release/manifest.py"

config_file="${HEX_TEAM_RELEASE_CONFIG:-$HOME/.config/hex/team-release.env}"
if [[ -f "$config_file" ]]; then
  set -a
  # shellcheck disable=SC1090
  source "$config_file"
  set +a
fi

channel="${HEX_BUILD_CHANNEL:-dev}"
app_name="${HEX_APP_NAME:-Slippy Hex}"
version=""
dry_run=0
skip_notarize=0
allow_dirty=0
skip_checks=0
rollback_manifest=""

usage() {
  cat >&2 <<'EOF'
Usage: scripts/team-release/release.sh [options]
  --dry-run                Build, package, and verify locally without uploading to R2
  --skip-notarize          Skip Apple notarization (only valid with --dry-run)
  --allow-dirty            Allow uncommitted changes (only valid with --dry-run)
  --skip-checks            Skip pre-build unit/identity tests (only valid with --dry-run)
  --channel <dev|stable>   Release channel (default: dev)
  --version <ver>          Override release version (default: 0.0.0-dev-<unix-ts>)
  --rollback <manifest>    Repoint the live channel manifest to an archived release
EOF
  exit 1
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --dry-run) dry_run=1; shift ;;
    --skip-notarize) skip_notarize=1; shift ;;
    --allow-dirty) allow_dirty=1; shift ;;
    --skip-checks) skip_checks=1; shift ;;
    --channel) channel="${2:-}"; shift 2 ;;
    --version) version="${2:-}"; shift 2 ;;
    --rollback) rollback_manifest="${2:-}"; shift 2 ;;
    -h|--help) usage ;;
    *) echo "Unknown option: $1" >&2; usage ;;
  esac
done

case "$channel" in
  dev|stable|beta) ;;
  *) echo "Unsupported channel: $channel" >&2; exit 1 ;;
esac

if [[ "$dry_run" -eq 0 && ( "$skip_notarize" -eq 1 || "$allow_dirty" -eq 1 || "$skip_checks" -eq 1 ) ]]; then
  echo "--skip-notarize, --allow-dirty, and --skip-checks are only permitted with --dry-run." >&2
  exit 1
fi

: "${HEX_RELEASE_ORIGIN:?Set HEX_RELEASE_ORIGIN to your HTTPS release host (or put it in ~/.config/hex/team-release.env)}"
origin="${HEX_RELEASE_ORIGIN%/}"
case "$origin" in
  https://*) ;;
  *) echo "HEX_RELEASE_ORIGIN must start with https:// (got $origin)" >&2; exit 1 ;;
esac

state_dir="${HEX_TEAM_RELEASE_STATE_DIR:-$HOME/.local/share/hex-team-releases}"
history_dir="$state_dir/history"
mkdir -p "$history_dir"

aws_r2() {
  local args=(--endpoint-url "$HEX_R2_ENDPOINT" --region auto)
  if [[ -n "${HEX_R2_AWS_PROFILE:-}" ]]; then
    args=(--profile "$HEX_R2_AWS_PROFILE" "${args[@]}")
  fi
  aws "${args[@]}" "$@"
}

require_r2() {
  : "${HEX_R2_BUCKET:?Set HEX_R2_BUCKET to your R2 bucket name}"
  : "${HEX_R2_ENDPOINT:?Set HEX_R2_ENDPOINT to https://<account-id>.r2.cloudflarestorage.com}"
  command -v aws >/dev/null 2>&1 || { echo "aws CLI is required for R2 uploads." >&2; exit 1; }
}

curl_release() {
  local headers=()
  if [[ -n "${HEX_RELEASE_CF_ACCESS_CLIENT_ID:-}" && -n "${HEX_RELEASE_CF_ACCESS_CLIENT_SECRET:-}" ]]; then
    headers+=(
      -H "CF-Access-Client-Id: $HEX_RELEASE_CF_ACCESS_CLIENT_ID"
      -H "CF-Access-Client-Secret: $HEX_RELEASE_CF_ACCESS_CLIENT_SECRET"
    )
  fi
  curl ${headers[@]+"${headers[@]}"} "$@"
}

fetch_live_json() {
  local url=$1
  local dest=$2
  if curl_release -fsSL --max-time 15 "$url" -o "$dest" 2>/dev/null; then
    return 0
  fi
  rm -f "$dest"
  return 1
}

upload_immutable_if_missing() {
  local local_file=$1
  local key=$2
  local expected_size
  expected_size="$(stat -f%z "$local_file")"
  local remote_size
  remote_size="$(aws_r2 s3api head-object --bucket "$HEX_R2_BUCKET" --key "$key" --query ContentLength --output text 2>/dev/null || true)"
  if [[ "$remote_size" == "$expected_size" ]]; then
    echo "Already uploaded: s3://$HEX_R2_BUCKET/$key ($expected_size bytes)"
    return 0
  fi
  aws_r2 s3 cp "$local_file" "s3://$HEX_R2_BUCKET/$key" \
    --cache-control "public, max-age=31536000, immutable" \
    --only-show-errors
}

verify_remote_artifact() {
  local url=$1
  local expected_size=$2
  local headers
  headers="$(curl_release -fsSI --max-time 30 "$url")"
  local remote_size
  remote_size="$(printf '%s\n' "$headers" | awk 'BEGIN{IGNORECASE=1} /^content-length:/ {gsub(/\r/, "", $2); print $2; exit}')"
  if [[ "$remote_size" != "$expected_size" ]]; then
    echo "Remote artifact size mismatch for $url: expected $expected_size, got ${remote_size:-none}" >&2
    exit 1
  fi
}

if [[ -n "$rollback_manifest" ]]; then
  [[ -f "$rollback_manifest" ]] || { echo "Archived manifest not found: $rollback_manifest" >&2; exit 1; }
  work="$(mktemp -d "${TMPDIR:-/tmp}/hex-team-rollback.XXXXXX")"
  trap 'rm -rf "$work"' EXIT HUP INT TERM

  live_manifest="$work/live.json"
  fetch_live_json "$origin/hex/desktop/$channel.json" "$live_manifest" || true
  next_seq="$(python3 "$manifest_py" next-sequence ${live_manifest:+--current "$live_manifest"})"
  rolled_manifest="$work/$channel.json"
  python3 "$manifest_py" rollback --from "$rollback_manifest" --sequence "$next_seq" --out "$rolled_manifest"

  target_version="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["release"]["version"])' "$rolled_manifest")"
  zip_url="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["release"]["targets"]["macos-arm64"]["url"])' "$rolled_manifest")"
  dmg_url="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["release"]["targets"]["macos-arm64-dmg"]["url"])' "$rolled_manifest")"
  zip_size="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["release"]["targets"]["macos-arm64"]["size"])' "$rolled_manifest")"
  dmg_size="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["release"]["targets"]["macos-arm64-dmg"]["size"])' "$rolled_manifest")"

  if [[ "$dry_run" -eq 1 ]]; then
    echo "Dry-run rollback manifest (version $target_version, sequence $next_seq):"
    cat "$rolled_manifest"
    exit 0
  fi

  require_r2
  verify_remote_artifact "$origin$zip_url" "$zip_size"
  verify_remote_artifact "$origin$dmg_url" "$dmg_size"
  aws_r2 s3 cp "$rolled_manifest" "s3://$HEX_R2_BUCKET/hex/desktop/$channel.json" \
    --content-type "application/json; charset=utf-8" \
    --cache-control "no-cache" \
    --only-show-errors
  live_version="$(curl_release -fsSL "$origin/hex/desktop/$channel.json" | python3 -c 'import json,sys; print(json.load(sys.stdin)["release"]["version"])')"
  [[ "$live_version" == "$target_version" ]] || { echo "Rollback verification failed: live version is $live_version" >&2; exit 1; }
  echo "Rolled back $channel channel to $target_version (sequence $next_seq)."
  exit 0
fi

[[ "$(uname -s)" == Darwin && "$(uname -m)" == arm64 ]] || {
  echo "Requires an Apple silicon Mac." >&2; exit 1;
}
if [[ "$allow_dirty" -eq 0 && -n "$(git -C "$root" status --porcelain)" ]]; then
  echo "Commit or remove working-tree changes before releasing (or pass --dry-run --allow-dirty)." >&2
  exit 1
fi

: "${HEX_TEAM_ID:?Set HEX_TEAM_ID to your Apple Developer Team ID}"
# shellcheck source=/dev/null
. "$root/scripts/macos-signing.sh"
identity="$(hex_codesign_identity "$HEX_TEAM_ID")"

if [[ "$skip_notarize" -eq 0 ]]; then
  if [[ -z "${HEX_NOTARY_PROFILE:-}" && ( -z "${HEX_NOTARY_KEY:-}" || -z "${HEX_NOTARY_KEY_ID:-}" || -z "${HEX_NOTARY_ISSUER:-}" ) ]]; then
    echo "Set HEX_NOTARY_PROFILE (recommended via xcrun notarytool store-credentials) or HEX_NOTARY_KEY/ID/ISSUER." >&2
    exit 1
  fi
fi

if [[ "$dry_run" -eq 0 ]]; then
  require_r2
fi

if [[ -z "$version" ]]; then
  version="0.0.0-dev-$(date -u +%s)"
fi
build_number="$(hex_build_number "$version")"
commit="$(git -C "$root" rev-parse HEAD)"
published_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

if [[ "$skip_checks" -eq 0 ]]; then
  env \
    -u HEX_APP_NAME \
    -u HEX_BUILD_CHANNEL \
    -u HEX_BUILD_NUMBER \
    -u HEX_DEV_NOTES_PAGE \
    -u HEX_DEV_NOTES_URL \
    -u HEX_LOCAL_BUILD \
    -u HEX_RELEASE_CF_ACCESS_CLIENT_ID \
    -u HEX_RELEASE_CF_ACCESS_CLIENT_SECRET \
    -u HEX_RELEASE_DOWNLOAD_URL \
    -u HEX_RELEASE_ORIGIN \
    -u HEX_TEST_BUILD \
    -u HEX_UPDATE_TEAM_ID \
    -u HEX_VERSION \
    "$root/scripts/test-app-identity.sh"
  env \
    -u HEX_APP_NAME \
    -u HEX_BUILD_CHANNEL \
    -u HEX_BUILD_NUMBER \
    -u HEX_DEV_NOTES_PAGE \
    -u HEX_DEV_NOTES_URL \
    -u HEX_LOCAL_BUILD \
    -u HEX_RELEASE_CF_ACCESS_CLIENT_ID \
    -u HEX_RELEASE_CF_ACCESS_CLIENT_SECRET \
    -u HEX_RELEASE_DOWNLOAD_URL \
    -u HEX_RELEASE_ORIGIN \
    -u HEX_TEST_BUILD \
    -u HEX_UPDATE_TEAM_ID \
    -u HEX_VERSION \
    "$root/scripts/check" updater:: dev_build:: labs:: app_paths::
fi

unset HEX_LOCAL_BUILD HEX_TEST_BUILD HEX_RELEASE_UNSIGNED HEX_COMMIT
export HEX_TEAM_ID
export HEX_APP_NAME="$app_name"
export HEX_UPDATE_TEAM_ID="$HEX_TEAM_ID"
export HEX_BUILD_CHANNEL="$channel"
export HEX_VERSION="$version"
export HEX_BUILD_NUMBER="$build_number"
export HEX_RELEASE_ORIGIN="$origin"
export HEX_RELEASE_DOWNLOAD_URL="$origin/download"
export HEX_DEV_NOTES_URL="$origin/hex/files/dev-notes.json"
export HEX_DEV_NOTES_PAGE="$origin/hex/dev"

bundle="$("$root/scripts/build-app.sh")"
"$root/scripts/validate-app.sh" "$bundle" "$app_name.app" "$version" "$build_number" >/dev/null

out_dir="$root/dist/team-release/$version"
mkdir -p "$out_dir"
artifact_stem="${app_name// /-}"
dmg_name="${artifact_stem}.dmg"
zip_name="${artifact_stem}.zip"
dmg="$out_dir/$dmg_name"
zip="$out_dir/$zip_name"
rm -f "$dmg" "$zip"

work="$(mktemp -d "${TMPDIR:-/tmp}/hex-team-release.XXXXXX")"
trap 'rm -rf "$work"' EXIT HUP INT TERM

requirement="$(sed "s/subject.OU] = \"5NZ4Q7NXJ4\"/subject.OU] = \"$HEX_TEAM_ID\"/" "$root/app/update-requirement.txt")"

if [[ "$skip_notarize" -eq 0 ]]; then
  "$root/scripts/notarize-app.sh" "$bundle" "$dmg" "$app_name"
  /usr/bin/ditto -c -k --sequesterRsrc --keepParent "$bundle" "$zip"
  codesign --verify --deep --strict -R="$requirement" "$bundle"
  spctl --assess --type execute --verbose=2 "$bundle"
else
  mkdir "$work/staging"
  /usr/bin/ditto "$bundle" "$work/staging/$app_name.app"
  ln -s /Applications "$work/staging/Applications"
  cp "$root/packaging/macos/dmg/background.tiff" "$work/staging/.background.tiff"
  cp "$root/packaging/macos/dmg/DS_Store" "$work/staging/.DS_Store"
  hdiutil create -quiet -fs APFS -format ULFO -volname "$app_name" -srcfolder "$work/staging" "$dmg"
  codesign --force --timestamp --sign "$identity" "$dmg"
  /usr/bin/ditto -c -k --sequesterRsrc --keepParent "$bundle" "$zip"
  dry_requirement="${requirement% and notarized}"
  codesign --verify --deep --strict -R="$dry_requirement" "$bundle"
fi

live_manifest="$work/live-manifest.json"
live_notes="$work/live-notes.json"
fetch_live_json "$origin/hex/desktop/$channel.json" "$live_manifest" || true
fetch_live_json "$origin/hex/files/dev-notes.json" "$live_notes" || true

next_seq="$(python3 "$manifest_py" next-sequence ${live_manifest:+--current "$live_manifest"})"
manifest_out="$out_dir/$channel.json"
python3 "$manifest_py" build \
  --channel "$channel" \
  --version "$version" \
  --sequence "$next_seq" \
  --zip "$zip" \
  --dmg "$dmg" \
  --zip-name "$zip_name" \
  --dmg-name "$dmg_name" \
  --notes-url "$origin/hex/dev" \
  --published-at "$published_at" \
  --out "$manifest_out"

titles_file="$work/titles.txt"
prev_commit=""
if [[ -f "$live_notes" ]]; then
  prev_commit="$(python3 -c 'import json,sys; builds=json.load(open(sys.argv[1])).get("builds",[]); print(builds[0].get("commit","") if builds else "")' "$live_notes" 2>/dev/null || true)"
fi
if [[ -n "$prev_commit" ]] && git -C "$root" cat-file -e "${prev_commit}^{commit}" 2>/dev/null; then
  git -C "$root" log --no-merges --pretty=format:%s "${prev_commit}..HEAD" > "$titles_file"
else
  git -C "$root" log -n 15 --no-merges --pretty=format:%s HEAD > "$titles_file"
fi

notes_out="$out_dir/dev-notes.json"
notes_args=(
  notes
  --version "$version"
  --commit "$commit"
  --published-at "$published_at"
  --titles-file "$titles_file"
  --out "$notes_out"
)
if [[ -f "$live_notes" ]]; then
  notes_args+=(--existing "$live_notes")
fi
python3 "$manifest_py" "${notes_args[@]}"

cp "$manifest_out" "$history_dir/$version.json"

if [[ "$dry_run" -eq 1 ]]; then
  printf 'Dry run succeeded.\n  Version:  %s\n  DMG:      %s\n  ZIP:      %s\n  Manifest: %s\n  Notes:    %s\n' \
    "$version" "$dmg" "$zip" "$manifest_out" "$notes_out"
  exit 0
fi

zip_sha="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["release"]["targets"]["macos-arm64"]["sha256"])' "$manifest_out")"
dmg_sha="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["release"]["targets"]["macos-arm64-dmg"]["sha256"])' "$manifest_out")"
zip_size="$(stat -f%z "$zip")"
dmg_size="$(stat -f%z "$dmg")"

upload_immutable_if_missing "$zip" "hex/files/$zip_sha/$zip_name"
upload_immutable_if_missing "$dmg" "hex/files/$dmg_sha/$dmg_name"

aws_r2 s3 cp "$notes_out" "s3://$HEX_R2_BUCKET/hex/files/dev-notes.json" \
  --content-type "application/json; charset=utf-8" \
  --cache-control "no-cache" \
  --only-show-errors

verify_remote_artifact "$origin/hex/files/$zip_sha/$zip_name" "$zip_size"
verify_remote_artifact "$origin/hex/files/$dmg_sha/$dmg_name" "$dmg_size"

aws_r2 s3 cp "$manifest_out" "s3://$HEX_R2_BUCKET/hex/desktop/$channel.json" \
  --content-type "application/json; charset=utf-8" \
  --cache-control "no-cache" \
  --only-show-errors

live_version="$(curl_release -fsSL "$origin/hex/desktop/$channel.json" | python3 -c 'import json,sys; print(json.load(sys.stdin)["release"]["version"])')"
[[ "$live_version" == "$version" ]] || { echo "Live manifest mismatch: expected $version, got $live_version" >&2; exit 1; }
download_location="$(curl_release -fsSI --max-time 15 "$origin/download" | awk 'BEGIN{IGNORECASE=1} /^location:/ {gsub(/\r/, "", $2); print $2; exit}')"
[[ "$download_location" == *"/hex/files/$dmg_sha/$dmg_name" ]] || {
  echo "Download redirect mismatch: got ${download_location:-none}" >&2
  exit 1
}

printf 'Published Hex %s (sequence %s) to %s\n  Download: %s/download\n  Rollback: scripts/team-release/release.sh --rollback %s\n' \
  "$version" "$next_seq" "$origin" "$origin" "$history_dir/$version.json"
