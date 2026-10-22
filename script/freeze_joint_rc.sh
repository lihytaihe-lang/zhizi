#!/usr/bin/env bash
set -euo pipefail

umask 077

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
RC_ROOT="$ROOT_DIR/build/rc"
INPUT_GUARD="$ROOT_DIR/script/rc_gate_support.sh"
CHECKSUM_TOOL="$ROOT_DIR/script/artifact_checksum.sh"
INPUT_BASENAME="Agent Input Method.inputmethod"
MANAGER_BASENAME="Agent Input Method.app"
METADATA_BASENAME="FROZEN_ARTIFACTS"
EXPECTED_VERSION="$(tr -d '[:space:]' < "$ROOT_DIR/VERSION")"
MINIMUM_BUILD=1020
AUTHORIZED_TEAM_ID="${AIM_AUTHORIZED_TEAM_ID-}"

usage() {
  echo "usage: $0 <signed-inputmethod-bundle> <signed-manager-bundle>" >&2
  echo "       $0 --verify-frozen <frozen-inputmethod-bundle> <frozen-manager-bundle>" >&2
  echo "       creating a freeze requires AIM_AUTHORIZED_TEAM_ID from the user-authorized signing identity" >&2
  exit 2
}

validate_team_id() {
  local team_id="$1"
  [[ "$team_id" =~ ^[A-Z0-9]{10}$ ]] || {
    echo "authorized TeamIdentifier must be exactly 10 uppercase ASCII letters/digits" >&2
    return 2
  }
}

if [[ -n "$AUTHORIZED_TEAM_ID" ]]; then
  validate_team_id "$AUTHORIZED_TEAM_ID"
fi

[[ -x "$INPUT_GUARD" && -x "$CHECKSUM_TOOL" ]] || {
  echo "RC freeze support scripts are missing or not executable" >&2
  exit 3
}

plist_value() {
  /usr/libexec/PlistBuddy -c "Print :$2" "$1/Contents/Info.plist" 2>/dev/null
}

required_plist_value() {
  local bundle="$1"
  local key="$2"
  local value
  value="$(plist_value "$bundle" "$key")" || {
    echo "missing required $key in bundle: $bundle" >&2
    return 3
  }
  [[ -n "$value" ]] || {
    echo "empty required $key in bundle: $bundle" >&2
    return 3
  }
  printf '%s\n' "$value"
}

signature_value() {
  local bundle="$1"
  local key="$2"
  /usr/bin/codesign -dv --verbose=4 "$bundle" 2>&1 \
    | /usr/bin/awk -F= -v key="$key" '
        $1 == key && !found { value=substr($0, length(key) + 2); found=1 }
        END { if (found) print value; else exit 3 }
      '
}

sha256_text() {
  /usr/bin/shasum -a 256 | /usr/bin/awk '{ print $1 }'
}

canonical_bundle() {
  local requested="$1"
  local label="$2"
  [[ -d "$requested" && ! -L "$requested" ]] || {
    echo "$label must be a real non-symlink directory: $requested" >&2
    return 2
  }
  local parent
  parent="$(cd "$(dirname "$requested")" && pwd -P)"
  printf '%s/%s\n' "$parent" "$(basename "$requested")"
}

assert_regular_tree() {
  local root="$1"
  local label="$2"
  local unexpected file_path link_count
  unexpected="$(/usr/bin/find "$root" ! -type d ! -type f -print -quit)"
  [[ -z "$unexpected" ]] || {
    echo "$label contains a symlink or special filesystem entry: $unexpected" >&2
    return 3
  }
  while IFS= read -r -d '' file_path; do
    link_count="$(/usr/bin/stat -f %l "$file_path")"
    [[ "$link_count" == "1" ]] || {
      echo "$label contains a hard-linked file: $file_path" >&2
      return 3
    }
  done < <(/usr/bin/find "$root" -type f -print0)
}

assert_no_user_write() {
  local root="$1"
  local writable
  writable="$(/usr/bin/find "$root" \
    \( -perm -0200 -o -perm -0020 -o -perm -0002 \) -print -quit)"
  [[ -z "$writable" ]] || {
    echo "frozen RC contains an entry with writable mode bits: $writable" >&2
    return 3
  }
  while IFS= read -r -d '' entry; do
    [[ ! -w "$entry" ]] || {
      echo "frozen RC contains an entry writable through effective permissions: $entry" >&2
      return 3
    }
  done < <(/usr/bin/find "$root" -print0)
}

assert_canonical_extended_metadata() {
  local root="$1"
  local entry attributes attribute
  while IFS= read -r -d '' entry; do
    if ! attributes="$(/usr/bin/xattr "$entry" 2>/dev/null)"; then
      echo "unable to inspect frozen RC extended attributes" >&2
      return 3
    fi
    while IFS= read -r attribute; do
      [[ -z "$attribute" \
        || "$attribute" == "com.apple.provenance" \
        || "$attribute" == "com.apple.macl" ]] || {
        echo "frozen RC contains a non-canonical extended attribute: $attribute" >&2
        return 3
      }
    done <<< "$attributes"
    if /bin/ls -lde "$entry" 2>/dev/null \
      | /usr/bin/awk 'NR > 1 && $1 ~ /^[0-9]+:$/ { found=1 } END { exit(found ? 0 : 1) }'; then
      echo "frozen RC contains an ACL: $entry" >&2
      return 3
    fi
  done < <(/usr/bin/find "$root" -print0)
}

verify_nested_identity() {
  local bundle="$1"
  local expected_team="$2"
  local expected_authority="$3"
  local file_path team authority
  while IFS= read -r file_path; do
    /usr/bin/file "$file_path" | /usr/bin/grep -q 'Mach-O' || continue
    /usr/bin/codesign --verify --strict --verbose=2 "$file_path"
    team="$(signature_value "$file_path" TeamIdentifier)"
    authority="$(signature_value "$file_path" Authority)"
    [[ "$team" == "$expected_team" && "$authority" == "$expected_authority" ]] || {
      echo "nested Mach-O signing identity mismatch: $file_path" >&2
      return 3
    }
  done < <(/usr/bin/find "$bundle/Contents" -type f -print | LC_ALL=C /usr/bin/sort)
}

# Populates PAIR_* globals only after the complete pair contract passes.
validate_pair_contract() {
  local input_bundle="$1"
  local manager_bundle="$2"
  local expected_team="${3:-}"
  local input_version manager_version input_build manager_build
  local input_id manager_id input_commit manager_commit input_inputs manager_inputs
  local input_configuration manager_configuration input_team manager_team
  local input_authority manager_authority verified_commit expected_inputs

  assert_regular_tree "$input_bundle" "inputmethod bundle"
  assert_regular_tree "$manager_bundle" "manager bundle"
  /usr/bin/codesign --verify --deep --strict --verbose=2 "$input_bundle"
  /usr/bin/codesign --verify --deep --strict --verbose=2 "$manager_bundle"

  input_version="$(required_plist_value "$input_bundle" CFBundleShortVersionString)"
  manager_version="$(required_plist_value "$manager_bundle" CFBundleShortVersionString)"
  input_build="$(required_plist_value "$input_bundle" CFBundleVersion)"
  manager_build="$(required_plist_value "$manager_bundle" CFBundleVersion)"
  input_id="$(required_plist_value "$input_bundle" CFBundleIdentifier)"
  manager_id="$(required_plist_value "$manager_bundle" CFBundleIdentifier)"
  input_commit="$(required_plist_value "$input_bundle" AgentIMEProductCommit)"
  manager_commit="$(required_plist_value "$manager_bundle" AgentIMEProductCommit)"
  input_inputs="$(required_plist_value "$input_bundle" AgentIMEProductInputsSHA256)"
  manager_inputs="$(required_plist_value "$manager_bundle" AgentIMEProductInputsSHA256)"
  input_configuration="$(required_plist_value "$input_bundle" AgentIMEBuildConfiguration)"
  manager_configuration="$(required_plist_value "$manager_bundle" AgentIMEBuildConfiguration)"
  input_team="$(signature_value "$input_bundle" TeamIdentifier)"
  manager_team="$(signature_value "$manager_bundle" TeamIdentifier)"
  input_authority="$(signature_value "$input_bundle" Authority)"
  manager_authority="$(signature_value "$manager_bundle" Authority)"

  [[ "$input_version" == "$EXPECTED_VERSION" && "$manager_version" == "$input_version" ]] || {
    echo "joint RC version mismatch or version is not $EXPECTED_VERSION" >&2
    return 3
  }
  [[ "$input_build" =~ ^[1-9][0-9]{0,8}$ && "$manager_build" == "$input_build" ]] || {
    echo "joint RC build metadata is invalid or mismatched" >&2
    return 3
  }
  (( 10#$input_build >= MINIMUM_BUILD )) || {
    echo "joint RC build must be at least $MINIMUM_BUILD: $input_build" >&2
    return 3
  }
  [[ "$input_id" == "com.hongye.facai.inputmethod.zhizi" \
    && "$manager_id" == "com.agentinputlayer.desktop" ]] || {
    echo "joint RC bundle identifiers are invalid" >&2
    return 3
  }
  [[ "$input_commit" =~ ^[0-9a-f]{40}$ && "$manager_commit" == "$input_commit" ]] || {
    echo "joint RC ProductCommit is invalid or mismatched" >&2
    return 3
  }
  [[ "$input_inputs" =~ ^[0-9a-f]{64}$ && "$manager_inputs" == "$input_inputs" ]] || {
    echo "joint RC ProductInputsSHA256 is invalid or mismatched" >&2
    return 3
  }
  [[ "$input_configuration" == "release" && "$manager_configuration" == "release" ]] || {
    echo "joint RC bundles must both be release builds" >&2
    return 3
  }
  validate_team_id "$input_team" || return 3
  [[ "$manager_team" == "$input_team" ]] || {
    echo "joint RC TeamIdentifier is mismatched" >&2
    return 3
  }
  if [[ -n "$expected_team" && "$input_team" != "$expected_team" ]]; then
    echo "joint RC TeamIdentifier does not match the user-authorized TeamIdentifier" >&2
    return 3
  fi
  [[ "$input_authority" == "Apple Development:"* && "$manager_authority" == "$input_authority" ]] || {
    echo "joint RC leaf signing authority is not matching Apple Development" >&2
    return 3
  }
  verify_nested_identity "$input_bundle" "$input_team" "$input_authority"
  verify_nested_identity "$manager_bundle" "$input_team" "$input_authority"

  verified_commit="$("$INPUT_GUARD" verify-inputs "$ROOT_DIR" "$input_commit")"
  [[ "$verified_commit" == "$input_commit" ]] || {
    echo "joint RC ProductCommit does not match the repository" >&2
    return 3
  }
  expected_inputs="$("$INPUT_GUARD" fingerprint "$ROOT_DIR" "$input_commit")"
  [[ "$expected_inputs" == "$input_inputs" ]] || {
    echo "joint RC ProductInputsSHA256 does not match ProductCommit" >&2
    return 3
  }

  PAIR_VERSION="$input_version"
  PAIR_BUILD="$input_build"
  PAIR_COMMIT="$input_commit"
  PAIR_INPUTS_SHA256="$input_inputs"
  PAIR_TEAM="$input_team"
  PAIR_TEAM_SHA256="$(printf '%s' "$input_team" | sha256_text)"
  PAIR_AUTHORITY="$input_authority"
  PAIR_AUTHORITY_SHA256="$(printf '%s' "$input_authority" | sha256_text)"
  PAIR_INPUT_SHA256="$("$CHECKSUM_TOOL" "$input_bundle")"
  PAIR_MANAGER_SHA256="$("$CHECKSUM_TOOL" "$manager_bundle")"
  [[ "$PAIR_INPUT_SHA256" =~ ^[0-9a-f]{64}$ && "$PAIR_MANAGER_SHA256" =~ ^[0-9a-f]{64}$ ]] || {
    echo "unable to checksum joint RC artifacts" >&2
    return 3
  }
}

emit_frozen_metadata() {
  printf 'format_version=1\n'
  printf 'artifact_version=%s\n' "$PAIR_VERSION"
  printf 'artifact_build=%s\n' "$PAIR_BUILD"
  printf 'product_commit=%s\n' "$PAIR_COMMIT"
  printf 'product_inputs_sha256=%s\n' "$PAIR_INPUTS_SHA256"
  printf 'team_identifier_sha256=%s\n' "$PAIR_TEAM_SHA256"
  printf 'leaf_authority_sha256=%s\n' "$PAIR_AUTHORITY_SHA256"
  printf 'input_basename=%s\n' "$INPUT_BASENAME"
  printf 'manager_basename=%s\n' "$MANAGER_BASENAME"
  printf 'input_artifact_sha256=%s\n' "$PAIR_INPUT_SHA256"
  printf 'manager_artifact_sha256=%s\n' "$PAIR_MANAGER_SHA256"
}

assert_exact_frozen_inventory() {
  local directory="$1"
  local entry basename entry_count=0
  while IFS= read -r -d '' entry; do
    entry_count=$((entry_count + 1))
    basename="$(basename "$entry")"
    case "$basename" in
      "$INPUT_BASENAME") [[ -d "$entry" && ! -L "$entry" ]] || return 3 ;;
      "$MANAGER_BASENAME") [[ -d "$entry" && ! -L "$entry" ]] || return 3 ;;
      "$METADATA_BASENAME") [[ -f "$entry" && ! -L "$entry" ]] || return 3 ;;
      *)
        echo "frozen RC directory contains an undeclared top-level entry: $entry" >&2
        return 3
        ;;
    esac
  done < <(/usr/bin/find "$directory" -mindepth 1 -maxdepth 1 -print0)
  [[ "$entry_count" -eq 3 ]] || {
    echo "frozen RC directory must contain exactly the two bundles and $METADATA_BASENAME" >&2
    return 3
  }
}

ensure_rc_root() {
  local create="$1"
  local build_root="$ROOT_DIR/build"
  local directory
  for directory in "$build_root" "$RC_ROOT"; do
    [[ ! -L "$directory" ]] || {
      echo "RC path may not contain a symlink: $directory" >&2
      return 3
    }
    if [[ ! -e "$directory" ]]; then
      [[ "$create" == "1" ]] || {
        echo "RC root is missing: $directory" >&2
        return 3
      }
      /bin/mkdir "$directory"
    fi
    [[ -d "$directory" && ! -L "$directory" ]] || {
      echo "RC path must be a real directory: $directory" >&2
      return 3
    }
  done
}

verify_frozen_pair() {
  local requested_input="$1"
  local requested_manager="$2"
  local input_bundle manager_bundle directory expected_directory metadata_file
  local frozen_team_sha256 frozen_authority_sha256
  input_bundle="$(canonical_bundle "$requested_input" "frozen inputmethod")"
  manager_bundle="$(canonical_bundle "$requested_manager" "frozen manager")"
  [[ "$(basename "$input_bundle")" == "$INPUT_BASENAME" \
    && "$(basename "$manager_bundle")" == "$MANAGER_BASENAME" ]] || {
    echo "frozen RC artifact basenames are invalid" >&2
    return 3
  }
  directory="$(dirname "$input_bundle")"
  [[ "$(dirname "$manager_bundle")" == "$directory" ]] || {
    echo "frozen RC pair must be in the same directory" >&2
    return 3
  }
  [[ -d "$directory" && ! -L "$directory" ]] || {
    echo "frozen RC directory must be a real directory" >&2
    return 3
  }
  ensure_rc_root 0
  assert_exact_frozen_inventory "$directory"
  assert_regular_tree "$directory" "frozen RC directory"
  assert_canonical_extended_metadata "$directory"
  assert_no_user_write "$directory"
  metadata_file="$directory/$METADATA_BASENAME"
  frozen_team_sha256="$(/usr/bin/awk -F= '
    $1 == "team_identifier_sha256" { count += 1; value = $2 }
    END { if (count == 1) print value; else exit 3 }
  ' "$metadata_file")" || {
    echo "$METADATA_BASENAME must contain exactly one TeamIdentifier hash" >&2
    return 3
  }
  frozen_authority_sha256="$(/usr/bin/awk -F= '
    $1 == "leaf_authority_sha256" { count += 1; value = $2 }
    END { if (count == 1) print value; else exit 3 }
  ' "$metadata_file")" || {
    echo "$METADATA_BASENAME must contain exactly one leaf Authority hash" >&2
    return 3
  }
  [[ "$frozen_team_sha256" =~ ^[0-9a-f]{64}$ \
    && "$frozen_authority_sha256" =~ ^[0-9a-f]{64}$ ]] || {
    echo "$METADATA_BASENAME signing identity hashes are invalid" >&2
    return 3
  }
  validate_pair_contract "$input_bundle" "$manager_bundle" ""
  [[ "$PAIR_TEAM_SHA256" == "$frozen_team_sha256" \
    && "$PAIR_AUTHORITY_SHA256" == "$frozen_authority_sha256" ]] || {
    echo "frozen RC signing identity hashes do not match the local bundle signatures" >&2
    return 3
  }
  if [[ -n "$AUTHORIZED_TEAM_ID" && "$PAIR_TEAM" != "$AUTHORIZED_TEAM_ID" ]]; then
    echo "frozen RC TeamIdentifier does not match AIM_AUTHORIZED_TEAM_ID" >&2
    return 3
  fi
  expected_directory="$RC_ROOT/${EXPECTED_VERSION}-build${PAIR_BUILD}"
  [[ "$directory" == "$expected_directory" ]] || {
    echo "joint gate only accepts repository-frozen RC pairs: expected=$expected_directory actual=$directory" >&2
    return 3
  }
  cmp -s "$metadata_file" <(emit_frozen_metadata) || {
    echo "$METADATA_BASENAME does not exactly match the frozen artifacts" >&2
    return 3
  }
  printf 'frozen_rc_dir=%s input_checksum=%s manager_checksum=%s build=%s\n' \
    "$directory" "$PAIR_INPUT_SHA256" "$PAIR_MANAGER_SHA256" "$PAIR_BUILD"
}

STAGE_ROOT=""
STAGE_INODE=""
LOCK_DIR=""
LOCK_TOKEN=""
LOCK_OWNED=0

cleanup() {
  local status="$?"
  trap - EXIT HUP INT TERM
  if [[ -n "$STAGE_ROOT" && "$STAGE_ROOT" == "$RC_ROOT"/.freeze-staging.* \
    && -d "$STAGE_ROOT" && ! -L "$STAGE_ROOT" \
    && "$(/usr/bin/stat -f %i "$STAGE_ROOT" 2>/dev/null || true)" == "$STAGE_INODE" ]]; then
    /bin/chmod -R u+w "$STAGE_ROOT" 2>/dev/null || true
    /bin/rm -rf "$STAGE_ROOT" || status=92
  fi
  if [[ "$LOCK_OWNED" == "1" && -n "$LOCK_DIR" && -d "$LOCK_DIR" && ! -L "$LOCK_DIR" \
    && -f "$LOCK_DIR/owner" && ! -L "$LOCK_DIR/owner" \
    && "$(<"$LOCK_DIR/owner")" == "$LOCK_TOKEN" ]]; then
    /bin/rm -f "$LOCK_DIR/owner" || status=92
    /bin/rmdir "$LOCK_DIR" 2>/dev/null || status=92
  fi
  exit "$status"
}

publish_exclusive() {
  local source="$1"
  local destination="$2"
  /bin/mkdir -p "$ROOT_DIR/build/swift-module-cache"
  /usr/bin/xcrun swift -module-cache-path "$ROOT_DIR/build/swift-module-cache" \
    - "$source" "$destination" <<'SWIFT'
import Darwin
import Foundation

let arguments = CommandLine.arguments
guard arguments.count == 3 else {
    fputs("exclusive RC publication requires source and destination\n", stderr)
    exit(2)
}
let result = arguments[1].withCString { source in
    arguments[2].withCString { destination in
        renameatx_np(AT_FDCWD, source, AT_FDCWD, destination, UInt32(RENAME_EXCL))
    }
}
if result != 0 {
    perror("exclusive RC publication")
    exit(1)
}
SWIFT
}

if [[ "${1:-}" == "--verify-frozen" ]]; then
  [[ $# -eq 3 ]] || usage
  verify_frozen_pair "$2" "$3"
  exit 0
fi

[[ $# -eq 2 ]] || usage
[[ -n "$AUTHORIZED_TEAM_ID" ]] || {
  echo "creating a frozen RC requires AIM_AUTHORIZED_TEAM_ID from the user-authorized signing identity" >&2
  exit 2
}
SOURCE_INPUT="$(canonical_bundle "$1" "inputmethod source")"
SOURCE_MANAGER="$(canonical_bundle "$2" "manager source")"
[[ "$SOURCE_INPUT" != "$SOURCE_MANAGER" ]] || {
  echo "joint RC source bundles must be distinct" >&2
  exit 2
}
validate_pair_contract "$SOURCE_INPUT" "$SOURCE_MANAGER" "$AUTHORIZED_TEAM_ID"
SOURCE_VERSION="$PAIR_VERSION"
SOURCE_BUILD="$PAIR_BUILD"
SOURCE_COMMIT="$PAIR_COMMIT"
SOURCE_INPUTS_SHA256="$PAIR_INPUTS_SHA256"
SOURCE_TEAM_SHA256="$PAIR_TEAM_SHA256"
SOURCE_AUTHORITY_SHA256="$PAIR_AUTHORITY_SHA256"
SOURCE_INPUT_SHA256="$PAIR_INPUT_SHA256"
SOURCE_MANAGER_SHA256="$PAIR_MANAGER_SHA256"

ensure_rc_root 1
TARGET_DIR="$RC_ROOT/${EXPECTED_VERSION}-build${SOURCE_BUILD}"
LOCK_DIR="$RC_ROOT/.freeze-${EXPECTED_VERSION}-build${SOURCE_BUILD}.lock"
LOCK_TOKEN="$$-$(/usr/bin/uuidgen)"
if [[ -e "$LOCK_DIR" || -L "$LOCK_DIR" ]] || ! /bin/mkdir "$LOCK_DIR" 2>/dev/null; then
  echo "another freeze owns the RC build lock, or it requires explicit recovery: $LOCK_DIR" >&2
  exit 3
fi
if ! printf '%s\n' "$LOCK_TOKEN" > "$LOCK_DIR/owner"; then
  if [[ -f "$LOCK_DIR/owner" && ! -L "$LOCK_DIR/owner" \
    && "$(<"$LOCK_DIR/owner")" == "$LOCK_TOKEN" ]]; then
    /bin/rm -f "$LOCK_DIR/owner" 2>/dev/null || true
  fi
  /bin/rmdir "$LOCK_DIR" 2>/dev/null || true
  exit 3
fi
LOCK_OWNED=1
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

if [[ -e "$TARGET_DIR" || -L "$TARGET_DIR" ]]; then
  [[ -d "$TARGET_DIR" && ! -L "$TARGET_DIR" ]] || {
    echo "frozen RC target exists but is not a real directory: $TARGET_DIR" >&2
    exit 3
  }
  verify_frozen_pair "$TARGET_DIR/$INPUT_BASENAME" "$TARGET_DIR/$MANAGER_BASENAME" >/dev/null
  [[ "$PAIR_VERSION" == "$SOURCE_VERSION" \
    && "$PAIR_BUILD" == "$SOURCE_BUILD" \
    && "$PAIR_COMMIT" == "$SOURCE_COMMIT" \
    && "$PAIR_INPUTS_SHA256" == "$SOURCE_INPUTS_SHA256" \
    && "$PAIR_TEAM_SHA256" == "$SOURCE_TEAM_SHA256" \
    && "$PAIR_AUTHORITY_SHA256" == "$SOURCE_AUTHORITY_SHA256" \
    && "$PAIR_INPUT_SHA256" == "$SOURCE_INPUT_SHA256" \
    && "$PAIR_MANAGER_SHA256" == "$SOURCE_MANAGER_SHA256" ]] || {
    echo "frozen RC build already exists with different artifacts; refusing overwrite" >&2
    exit 3
  }
  printf '%s\n%s\n' "$TARGET_DIR/$INPUT_BASENAME" "$TARGET_DIR/$MANAGER_BASENAME"
  exit 0
fi

STAGE_ROOT="$(/usr/bin/mktemp -d "$RC_ROOT/.freeze-staging.${EXPECTED_VERSION}-build${SOURCE_BUILD}.XXXXXX")"
STAGE_INODE="$(/usr/bin/stat -f %i "$STAGE_ROOT")"
[[ "$(/usr/bin/stat -f %d "$STAGE_ROOT")" == "$(/usr/bin/stat -f %d "$RC_ROOT")" ]] || {
  echo "RC staging and publication target must share a filesystem" >&2
  exit 3
}
/usr/bin/ditto --norsrc --noextattr --noqtn --noacl \
  "$SOURCE_INPUT" "$STAGE_ROOT/$INPUT_BASENAME"
/usr/bin/ditto --norsrc --noextattr --noqtn --noacl \
  "$SOURCE_MANAGER" "$STAGE_ROOT/$MANAGER_BASENAME"

validate_pair_contract "$STAGE_ROOT/$INPUT_BASENAME" "$STAGE_ROOT/$MANAGER_BASENAME" "$AUTHORIZED_TEAM_ID"
[[ "$PAIR_VERSION" == "$SOURCE_VERSION" \
  && "$PAIR_BUILD" == "$SOURCE_BUILD" \
  && "$PAIR_COMMIT" == "$SOURCE_COMMIT" \
  && "$PAIR_INPUTS_SHA256" == "$SOURCE_INPUTS_SHA256" \
  && "$PAIR_TEAM_SHA256" == "$SOURCE_TEAM_SHA256" \
  && "$PAIR_AUTHORITY_SHA256" == "$SOURCE_AUTHORITY_SHA256" \
  && "$PAIR_INPUT_SHA256" == "$SOURCE_INPUT_SHA256" \
  && "$PAIR_MANAGER_SHA256" == "$SOURCE_MANAGER_SHA256" ]] || {
  echo "staged RC pair differs from the validated source pair" >&2
  exit 3
}
[[ "$("$CHECKSUM_TOOL" "$SOURCE_INPUT")" == "$SOURCE_INPUT_SHA256" \
  && "$("$CHECKSUM_TOOL" "$SOURCE_MANAGER")" == "$SOURCE_MANAGER_SHA256" ]] || {
  echo "source RC pair changed during freeze" >&2
  exit 3
}
emit_frozen_metadata > "$STAGE_ROOT/$METADATA_BASENAME"
assert_exact_frozen_inventory "$STAGE_ROOT"
assert_regular_tree "$STAGE_ROOT" "staged frozen RC directory"
/bin/chmod -RN "$STAGE_ROOT"
/usr/bin/xattr -cr "$STAGE_ROOT"
assert_canonical_extended_metadata "$STAGE_ROOT"
/bin/chmod -R a-w "$STAGE_ROOT"
assert_no_user_write "$STAGE_ROOT"
/usr/bin/codesign --verify --deep --strict --verbose=2 "$STAGE_ROOT/$INPUT_BASENAME"
/usr/bin/codesign --verify --deep --strict --verbose=2 "$STAGE_ROOT/$MANAGER_BASENAME"
[[ "$("$CHECKSUM_TOOL" "$STAGE_ROOT/$INPUT_BASENAME")" == "$SOURCE_INPUT_SHA256" \
  && "$("$CHECKSUM_TOOL" "$STAGE_ROOT/$MANAGER_BASENAME")" == "$SOURCE_MANAGER_SHA256" ]] || {
  echo "frozen RC checksum changed after removing write permissions" >&2
  exit 3
}

if ! publish_exclusive "$STAGE_ROOT" "$TARGET_DIR"; then
  if [[ -d "$TARGET_DIR" && ! -L "$TARGET_DIR" ]] \
    && verify_frozen_pair "$TARGET_DIR/$INPUT_BASENAME" "$TARGET_DIR/$MANAGER_BASENAME" >/dev/null \
    && [[ "$PAIR_INPUT_SHA256" == "$SOURCE_INPUT_SHA256" \
      && "$PAIR_MANAGER_SHA256" == "$SOURCE_MANAGER_SHA256" \
      && "$PAIR_COMMIT" == "$SOURCE_COMMIT" \
      && "$PAIR_INPUTS_SHA256" == "$SOURCE_INPUTS_SHA256" ]]; then
    printf '%s\n%s\n' "$TARGET_DIR/$INPUT_BASENAME" "$TARGET_DIR/$MANAGER_BASENAME"
    exit 0
  fi
  echo "exclusive frozen RC publication failed; existing target was not modified" >&2
  exit 3
fi
STAGE_ROOT=""
STAGE_INODE=""
verify_frozen_pair "$TARGET_DIR/$INPUT_BASENAME" "$TARGET_DIR/$MANAGER_BASENAME" >/dev/null
printf '%s\n%s\n' "$TARGET_DIR/$INPUT_BASENAME" "$TARGET_DIR/$MANAGER_BASENAME"
