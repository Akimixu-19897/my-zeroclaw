#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$ROOT_DIR"

detect_profile() {
  local profile="debug"
  local args=("$@")
  local i=0
  while [ $i -lt ${#args[@]} ]; do
    case "${args[$i]}" in
      --release)
        profile="release"
        ;;
      --profile)
        i=$((i + 1))
        if [ $i -lt ${#args[@]} ]; then
          profile="${args[$i]}"
        fi
        ;;
      --profile=*)
        profile="${args[$i]#--profile=}"
        ;;
    esac
    i=$((i + 1))
  done
  printf '%s\n' "$profile"
}

safe_du_bytes() {
  local path="$1"
  if [ ! -e "$path" ]; then
    printf '0\n'
    return
  fi
  du -sk "$path" 2>/dev/null | awk '{print $1 * 1024}'
}

human_bytes() {
  local bytes="$1"
  local units=(B KB MB GB TB)
  local unit_index=0
  local value="$bytes"
  while [ "$value" -ge 1024 ] && [ $unit_index -lt $((${#units[@]} - 1)) ]; do
    value=$((value / 1024))
    unit_index=$((unit_index + 1))
  done
  printf '%s%s\n' "$value" "${units[$unit_index]}"
}

cleanup_profile_artifacts() {
  local profile="$1"
  local profile_dir="$ROOT_DIR/target/$profile"
  local deps_dir="$profile_dir/deps"
  local incremental_dir="$profile_dir/incremental"
  local tmp_dir="$ROOT_DIR/target/tmp"

  local before_profile_bytes before_deps_bytes before_incremental_bytes
  before_profile_bytes="$(safe_du_bytes "$profile_dir")"
  before_deps_bytes="$(safe_du_bytes "$deps_dir")"
  before_incremental_bytes="$(safe_du_bytes "$incremental_dir")"

  rm -rf "$incremental_dir" "$tmp_dir"

  if [ -d "$deps_dir" ]; then
    find "$deps_dir" -maxdepth 1 -type f \
      ! -name 'lib*' \
      ! -name '*.d' \
      ! -name '*.rlib' \
      ! -name '*.rmeta' \
      ! -name '*.so' \
      ! -name '*.dylib' \
      ! -name '*.dll' \
      -delete
  fi

  local after_profile_bytes after_deps_bytes after_incremental_bytes reclaimed_bytes
  after_profile_bytes="$(safe_du_bytes "$profile_dir")"
  after_deps_bytes="$(safe_du_bytes "$deps_dir")"
  after_incremental_bytes="$(safe_du_bytes "$incremental_dir")"

  reclaimed_bytes=$((before_profile_bytes - after_profile_bytes))
  if [ "$reclaimed_bytes" -lt 0 ]; then
    reclaimed_bytes=0
  fi

  cat <<EOF
Cleanup complete for target/$profile
  deps:        $(human_bytes "$before_deps_bytes") -> $(human_bytes "$after_deps_bytes")
  incremental: $(human_bytes "$before_incremental_bytes") -> $(human_bytes "$after_incremental_bytes")
  reclaimed:   $(human_bytes "$reclaimed_bytes")
EOF
}

print_help() {
  cat <<'EOF'
Usage: ./cargo-build-clean.sh [cargo build args...]

Runs `cargo build` and, if it succeeds, removes the heaviest disposable build
artifacts for the selected profile:
- target/<profile>/incremental
- executable test/build outputs under target/<profile>/deps

Examples:
  ./cargo-build-clean.sh
  ./cargo-build-clean.sh --release
  ./cargo-build-clean.sh --features rag-pdf
  ./cargo-build-clean.sh --profile dev-fast
EOF
}

if [ "${1:-}" = "-h" ] || [ "${1:-}" = "--help" ]; then
  print_help
  exit 0
fi

PROFILE="$(detect_profile "$@")"

echo "==> cargo build $*"
cargo build "$@"

echo
cleanup_profile_artifacts "$PROFILE"
