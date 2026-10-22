#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEPS_DIR="$ROOT_DIR/.build-deps"
RIME_VERSION="1.16.1"
RIME_COMMIT="de4700e9f6b75b109910613df907965e3cbe0567"
RIME_ASSET_ID="de4700e"
RIME_ARCHIVE_SHA256="147dc220d20bcf2650889c98f943f1792b3c675dbef91f42f9151a216ad2c372"
RIME_DYLIB_SHA256="bdaea3fbf4bb7600d8cc4a1b05db38a286bd9b84e7f5940d20ece5715943d4e1"
RIME_API_SHA256="6eb5629162c44761e7b878f1b122f464de5c8685bd3cd5ac1e8519badc6d6424"
RIME_API_STDBOOL_SHA256="f5a64254e98c1c12ce6628f5d540bcd4e4b542ec287b4873c0b0ce4ed1bece99"
RIME_LICENSE_SHA256="f67d27a6d2d586fcfed4b4c886a83747095396a39b6641e18e855086be2ec400"
RIME_LOCKED_VERSION_SHA256="61a978be63d14f884bb4313f18d59915f35147177eb1441578a66614ba6b0212"
PINYIN_COMMIT="0c6861ef7420ee780270ca6d993d18d4101049d0"
PINYIN_ARCHIVE_SHA256="46f37114a7929ecc01003a236803c8b1e5198382e6a21f83fae036604a6b08bf"
PINYIN_DICT_SHA256="e341598343a0f0f2035bb1aafc34a7f3bb7887deeecb3f60796262aaa2983e6b"
PINYIN_LICENSE_SHA256="cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30"
PINYIN_AUTHORS_SHA256="f4cff0fcbca4668ac449c24a53be547e162bc60cce63fdc5d5906801a452edc4"
PINYIN_LOCKED_VERSION_SHA256="8d334a90f5fe43592d2b1f05dc1e45b20294bbad85c27600f91b456ffa832b24"
RUST_VERSION="1.94.1"
RUST_COMMIT="e408947bfd200af42db322daf0fadfe7e26d3bd1"
RUST_LICENSE_APACHE_SHA256="a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2"
RUST_LICENSE_MIT_SHA256="23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3"
RUST_COPYRIGHT_LIBRARY_SHA256="af70aaabed1b73e872f14f9130db37e09f3f4d73a5f7c598b9173697a5d2729f"
RUST_LOCKED_VERSION_SHA256="04e2429f47a5ec78fb87da7eba9a07190ff81d9b61540f499b2254a3d6f1ed3f"
RIME_DIR="$DEPS_DIR/librime/$RIME_VERSION"
PINYIN_DIR="$DEPS_DIR/rime-pinyin-simp/$PINYIN_COMMIT"
RUST_LICENSE_DIR="$DEPS_DIR/rust/$RUST_VERSION/licenses"

verify_sha256() {
  local file="$1"
  local expected="$2"
  local actual
  if [[ ! -f "$file" || -L "$file" ]]; then
    echo "missing or non-regular locked file: $file" >&2
    return 1
  fi
  actual="$(shasum -a 256 "$file" | awk '{print $1}')" || return 1
  if [[ "$actual" != "$expected" ]]; then
    echo "checksum mismatch for $file: expected $expected, got $actual" >&2
    return 1
  fi
}

validate_rime_dir() {
  local directory="$1"
  local architectures
  [[ -d "$directory" && ! -L "$directory" ]] || {
    echo "missing or invalid librime cache directory: $directory" >&2
    return 1
  }
  verify_sha256 "$directory/lib/librime.1.dylib" "$RIME_DYLIB_SHA256" || return 1
  verify_sha256 "$directory/include/rime_api.h" "$RIME_API_SHA256" || return 1
  verify_sha256 "$directory/include/rime_api_stdbool.h" "$RIME_API_STDBOOL_SHA256" || return 1
  verify_sha256 "$directory/licenses/LICENSE.librime" "$RIME_LICENSE_SHA256" || return 1
  verify_sha256 "$directory/LOCKED_VERSION" "$RIME_LOCKED_VERSION_SHA256" || return 1
  architectures="$(lipo -archs "$directory/lib/librime.1.dylib")" || return 1
  [[ "$architectures" == "arm64" ]] || {
    echo "unexpected librime architectures: $architectures" >&2
    return 1
  }
  [[ -L "$directory/lib/librime.dylib" ]] || {
    echo "missing locked librime linker symlink" >&2
    return 1
  }
  [[ "$(readlink "$directory/lib/librime.dylib")" == "librime.1.dylib" ]] || {
    echo "unexpected librime linker symlink target" >&2
    return 1
  }
  [[ -z "$(find "$directory" -type l ! -path "$directory/lib/librime.dylib" -print -quit)" ]] || {
    echo "unexpected symlink in librime cache: $directory" >&2
    return 1
  }
}

acquire_bootstrap_lock() {
  local lock_directory="$1"
  local recorded_pid stale_directory

  if [[ -L "$lock_directory" ]]; then
    echo "refusing symlink bootstrap lock: $lock_directory" >&2
    return 4
  fi
  if mkdir "$lock_directory" 2>/dev/null; then
    printf '%s\n' "$$" > "$lock_directory/pid"
    return 0
  fi
  if [[ -L "$lock_directory" || ! -d "$lock_directory" ]]; then
    echo "refusing abnormal bootstrap lock: $lock_directory" >&2
    return 4
  fi

  recorded_pid=""
  if [[ -f "$lock_directory/pid" && ! -L "$lock_directory/pid" ]]; then
    IFS= read -r recorded_pid < "$lock_directory/pid" || true
  fi
  if [[ "$recorded_pid" =~ ^[1-9][0-9]*$ ]] && kill -0 "$recorded_pid" 2>/dev/null; then
    echo "native dependency bootstrap is already running with pid $recorded_pid" >&2
    return 4
  fi

  stale_directory="$DEPS_DIR/.bootstrap-native.lock.stale.$$"
  if [[ -e "$stale_directory" || -L "$stale_directory" ]]; then
    echo "refusing pre-existing stale-lock recovery path: $stale_directory" >&2
    return 4
  fi
  if ! mv "$lock_directory" "$stale_directory" 2>/dev/null; then
    echo "bootstrap lock changed during stale-lock recovery" >&2
    return 4
  fi
  if ! mkdir "$lock_directory" 2>/dev/null; then
    rm -rf "$stale_directory"
    echo "another process acquired the bootstrap lock during recovery" >&2
    return 4
  fi
  printf '%s\n' "$$" > "$lock_directory/pid"
  rm -rf "$stale_directory"
}

validate_pinyin_dir() {
  local directory="$1"
  [[ -d "$directory" && ! -L "$directory" ]] || {
    echo "missing or invalid pinyin cache directory: $directory" >&2
    return 1
  }
  [[ -z "$(find "$directory" -type l -print -quit)" ]] || {
    echo "unexpected symlink in pinyin cache: $directory" >&2
    return 1
  }
  verify_sha256 "$directory/pinyin_simp.dict.yaml" "$PINYIN_DICT_SHA256" || return 1
  verify_sha256 "$directory/LICENSE" "$PINYIN_LICENSE_SHA256" || return 1
  verify_sha256 "$directory/AUTHORS" "$PINYIN_AUTHORS_SHA256" || return 1
  verify_sha256 "$directory/LOCKED_VERSION" "$PINYIN_LOCKED_VERSION_SHA256" || return 1
}

validate_rust_license_dir() {
  local directory="$1"
  [[ -d "$directory" && ! -L "$directory" ]] || {
    echo "missing or invalid Rust license cache directory: $directory" >&2
    return 1
  }
  [[ -z "$(find "$directory" -type l -print -quit)" ]] || {
    echo "unexpected symlink in Rust license cache: $directory" >&2
    return 1
  }
  verify_sha256 "$directory/LICENSE-APACHE" "$RUST_LICENSE_APACHE_SHA256" || return 1
  verify_sha256 "$directory/LICENSE-MIT" "$RUST_LICENSE_MIT_SHA256" || return 1
  verify_sha256 "$directory/COPYRIGHT-library.html" "$RUST_COPYRIGHT_LIBRARY_SHA256" || return 1
  verify_sha256 "$directory/LOCKED_VERSION" "$RUST_LOCKED_VERSION_SHA256" || return 1
}

rebuild_rust_license_cache() (
  set -euo pipefail
  local parent candidate sysroot verbose commit version apache mit copyright
  parent="$(dirname "$RUST_LICENSE_DIR")"
  mkdir -p "$parent"
  candidate="$(mktemp -d "$parent/.bootstrap-rust-license.XXXXXX")"
  trap 'rm -rf "$candidate"' EXIT
  sysroot="$(rustc --print sysroot)"
  verbose="$(rustc --version --verbose)"
  version="$(printf '%s\n' "$verbose" | awk '$1 == "release:" { print $2; exit }')"
  commit="$(printf '%s\n' "$verbose" | awk '$1 == "commit-hash:" { print $2; exit }')"
  [[ "$version" == "$RUST_VERSION" && "$commit" == "$RUST_COMMIT" ]] || {
    echo "Rust toolchain does not match locked license provenance: release=$version commit=$commit" >&2
    return 1
  }
  apache="$sysroot/share/doc/rust/html/static.files/LICENSE-APACHE-a60eea81.txt"
  mit="$sysroot/share/doc/rust/html/static.files/LICENSE-MIT-23f18e03.txt"
  copyright="$sysroot/share/doc/rust/COPYRIGHT-library.html"
  verify_sha256 "$apache" "$RUST_LICENSE_APACHE_SHA256"
  verify_sha256 "$mit" "$RUST_LICENSE_MIT_SHA256"
  verify_sha256 "$copyright" "$RUST_COPYRIGHT_LIBRARY_SHA256"
  cp "$apache" "$candidate/LICENSE-APACHE"
  cp "$mit" "$candidate/LICENSE-MIT"
  cp "$copyright" "$candidate/COPYRIGHT-library.html"
  printf '%s\n' "$RUST_VERSION $RUST_COMMIT" > "$candidate/LOCKED_VERSION"
  validate_rust_license_dir "$candidate"
  replace_directory "$candidate" "$RUST_LICENSE_DIR"
  trap - EXIT
)

replace_directory() (
  set -euo pipefail
  local candidate="$1"
  local destination="$2"
  local parent backup had_previous
  parent="$(dirname "$destination")"
  backup="$(mktemp -d "$parent/.$(basename "$destination").backup.XXXXXX")" || return 1
  if ! rmdir "$backup"; then
    return 1
  fi
  had_previous=0

  restore_previous() {
    local status="$?"
    trap - EXIT HUP INT TERM
    if [[ "$status" -ne 0 && "$had_previous" -eq 1 && ! -e "$destination" && ! -L "$destination" ]]; then
      mv "$backup" "$destination" || true
    fi
    exit "$status"
  }
  trap restore_previous EXIT
  trap 'exit 129' HUP
  trap 'exit 130' INT
  trap 'exit 143' TERM

  if [[ -e "$destination" || -L "$destination" ]]; then
    if ! mv "$destination" "$backup"; then
      trap - EXIT HUP INT TERM
      return 1
    fi
    had_previous=1
  fi
  if ! mv "$candidate" "$destination"; then
    if [[ "$had_previous" -eq 1 ]]; then
      if ! mv "$backup" "$destination"; then
        echo "failed to restore dependency cache after replacement error: $destination" >&2
        trap - EXIT HUP INT TERM
        return 5
      fi
    fi
    trap - EXIT HUP INT TERM
    return 1
  fi
  if [[ "$had_previous" -eq 1 ]]; then
    if ! rm -rf "$backup"; then
      echo "new dependency cache is active but old backup could not be removed: $backup" >&2
      trap - EXIT HUP INT TERM
      return 1
    fi
  fi
  trap - EXIT HUP INT TERM
)

rebuild_rime() (
  set -euo pipefail
  local parent work_dir archive candidate
  parent="$(dirname "$RIME_DIR")"
  mkdir -p "$parent"
  work_dir="$(mktemp -d "$parent/.bootstrap-$RIME_VERSION.XXXXXX")"
  cleanup_rime_staging() {
    local status="$?"
    trap - EXIT HUP INT TERM
    rm -rf "$work_dir"
    exit "$status"
  }
  trap cleanup_rime_staging EXIT
  trap 'exit 129' HUP
  trap 'exit 130' INT
  trap 'exit 143' TERM
  archive="$work_dir/librime.tar.bz2"
  candidate="$work_dir/candidate"

  curl --fail --location --silent --show-error \
    --output "$archive" \
    "https://github.com/rime/librime/releases/download/$RIME_VERSION/rime-$RIME_ASSET_ID-macOS-universal.tar.bz2"
  verify_sha256 "$archive" "$RIME_ARCHIVE_SHA256"
  tar -xjf "$archive" -C "$work_dir"
  mkdir -p "$candidate/include" "$candidate/lib" "$candidate/licenses"
  cp "$work_dir/dist/include/rime_api.h" "$candidate/include/rime_api.h"
  cp "$work_dir/dist/include/rime_api_stdbool.h" "$candidate/include/rime_api_stdbool.h"
  lipo "$work_dir/dist/lib/librime.1.16.1.dylib" -thin arm64 -output "$candidate/lib/librime.1.dylib"
  ln -s librime.1.dylib "$candidate/lib/librime.dylib"
  curl --fail --location --silent --show-error \
    --output "$candidate/licenses/LICENSE.librime" \
    "https://raw.githubusercontent.com/rime/librime/$RIME_COMMIT/LICENSE"
  printf '%s\n' "$RIME_VERSION $RIME_COMMIT $RIME_ARCHIVE_SHA256" > "$candidate/LOCKED_VERSION"
  validate_rime_dir "$candidate"
  replace_directory "$candidate" "$RIME_DIR"
  rm -rf "$work_dir"
  trap - EXIT HUP INT TERM
)

rebuild_pinyin() (
  set -euo pipefail
  local parent work_dir archive source_dir candidate
  parent="$(dirname "$PINYIN_DIR")"
  mkdir -p "$parent"
  work_dir="$(mktemp -d "$parent/.bootstrap-$PINYIN_COMMIT.XXXXXX")"
  cleanup_pinyin_staging() {
    local status="$?"
    trap - EXIT HUP INT TERM
    rm -rf "$work_dir"
    exit "$status"
  }
  trap cleanup_pinyin_staging EXIT
  trap 'exit 129' HUP
  trap 'exit 130' INT
  trap 'exit 143' TERM
  archive="$work_dir/pinyin.tar.gz"
  candidate="$work_dir/candidate"

  curl --fail --location --silent --show-error \
    --output "$archive" \
    "https://codeload.github.com/rime/rime-pinyin-simp/tar.gz/$PINYIN_COMMIT"
  verify_sha256 "$archive" "$PINYIN_ARCHIVE_SHA256"
  tar -xzf "$archive" -C "$work_dir"
  source_dir="$work_dir/rime-pinyin-simp-$PINYIN_COMMIT"
  mkdir -p "$candidate"
  cp "$source_dir/pinyin_simp.dict.yaml" "$candidate/pinyin_simp.dict.yaml"
  cp "$source_dir/LICENSE" "$candidate/LICENSE"
  cp "$source_dir/AUTHORS" "$candidate/AUTHORS"
  printf '%s\n' "$PINYIN_COMMIT $PINYIN_ARCHIVE_SHA256" > "$candidate/LOCKED_VERSION"
  validate_pinyin_dir "$candidate"
  replace_directory "$candidate" "$PINYIN_DIR"
  rm -rf "$work_dir"
  trap - EXIT HUP INT TERM
)

PYPINYIN_VERSION="0.55.0"
PYPINYIN_WHEEL_SHA256="d53b1e8ad2cdb815fb2cb604ed3123372f5a28c6f447571244aca36fc62a286f"
PYPINYIN_DIR="$DEPS_DIR/pypinyin/$PYPINYIN_VERSION"

# The dictionary checks (check_idiom_dictionary.sh, check_modern_dictionary.sh) run
# the pinned pypinyin from this venv; install it hash-locked so a source build works.
ensure_pypinyin() {
  local python="$PYPINYIN_DIR/venv/bin/python" requirements
  if [[ -x "$python" ]] && [[ "$("$python" -c 'import pypinyin; print(pypinyin.__version__)' 2>/dev/null)" == "$PYPINYIN_VERSION" ]]; then
    echo "verified cached pypinyin $PYPINYIN_VERSION"
    return 0
  fi
  echo "installing pypinyin $PYPINYIN_VERSION (hash-locked)" >&2
  rm -rf "$PYPINYIN_DIR"
  mkdir -p "$PYPINYIN_DIR"
  python3 -m venv "$PYPINYIN_DIR/venv"
  requirements="$PYPINYIN_DIR/requirements.txt"
  printf 'pypinyin==%s --hash=sha256:%s\n' "$PYPINYIN_VERSION" "$PYPINYIN_WHEEL_SHA256" > "$requirements"
  "$python" -m pip install --quiet --disable-pip-version-check --require-hashes --only-binary=:all: -r "$requirements"
  [[ "$("$python" -c 'import pypinyin; print(pypinyin.__version__)')" == "$PYPINYIN_VERSION" ]]
}

main() {
  local lock_directory
  mkdir -p "$DEPS_DIR"
  lock_directory="$DEPS_DIR/.bootstrap-native.lock"
  acquire_bootstrap_lock "$lock_directory"
  release_bootstrap_lock() {
    local status="$?"
    trap - EXIT HUP INT TERM
    rm -rf "$lock_directory"
    exit "$status"
  }
  trap release_bootstrap_lock EXIT
  trap 'exit 129' HUP
  trap 'exit 130' INT
  trap 'exit 143' TERM

  if validate_rime_dir "$RIME_DIR"; then
    echo "verified cached librime $RIME_VERSION"
  else
    echo "rebuilding invalid or incomplete librime cache" >&2
    rebuild_rime
    validate_rime_dir "$RIME_DIR"
  fi

  if validate_pinyin_dir "$PINYIN_DIR"; then
    echo "verified cached rime-pinyin-simp $PINYIN_COMMIT"
  else
    echo "rebuilding invalid or incomplete pinyin cache" >&2
    rebuild_pinyin
    validate_pinyin_dir "$PINYIN_DIR"
  fi

  if validate_rust_license_dir "$RUST_LICENSE_DIR"; then
    echo "verified cached Rust standard-library notices $RUST_VERSION"
  else
    echo "rebuilding invalid or incomplete Rust standard-library notice cache" >&2
    rebuild_rust_license_cache
    validate_rust_license_dir "$RUST_LICENSE_DIR"
  fi

  ensure_pypinyin

  file "$RIME_DIR/lib/librime.1.dylib"
  otool -L "$RIME_DIR/lib/librime.1.dylib"
  shasum -a 256 \
    "$RIME_DIR/LOCKED_VERSION" \
    "$RIME_DIR/lib/librime.1.dylib" \
    "$RIME_DIR/include/rime_api.h" \
    "$RIME_DIR/include/rime_api_stdbool.h" \
    "$RIME_DIR/licenses/LICENSE.librime" \
    "$PINYIN_DIR/LOCKED_VERSION" \
    "$PINYIN_DIR/pinyin_simp.dict.yaml" \
    "$PINYIN_DIR/LICENSE" \
    "$PINYIN_DIR/AUTHORS" \
    "$RUST_LICENSE_DIR/LOCKED_VERSION" \
    "$RUST_LICENSE_DIR/LICENSE-APACHE" \
    "$RUST_LICENSE_DIR/LICENSE-MIT" \
    "$RUST_LICENSE_DIR/COPYRIGHT-library.html"

  rm -rf "$lock_directory"
  trap - EXIT HUP INT TERM
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  main "$@"
fi
