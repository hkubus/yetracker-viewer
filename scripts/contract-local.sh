#!/usr/bin/env bash
# Runs the API contract suite (`pnpm test:contract`) against a throwaway server: a fresh temporary STORAGE_DIR, the
# catalog imported from the offline fixture sheet, downloads and periodic syncs off. The server and the temporary
# directory are removed on exit, whatever happens.
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: scripts/contract-local.sh [--build] [--release]

  --build    run `cargo build` for the chosen profile first (by default the existing binary is used as is)
  --release  use target/release/yetracker-api instead of target/debug/yetracker-api

Environment:
  API_BIN           API binary to run (overrides the profile)
  CONTRACT_PORT     port for the server (default: a free port chosen by the OS)
  CONTRACT_TIMEOUT  seconds to wait for the fixture import (default: 60)
  MEDIA_TOOLS       passed through to the media tests (present | absent)
EOF
}

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
manifest="$repo_root/apps/api-rs/Cargo.toml"
fixture="$repo_root/apps/api-rs/tests/fixtures/sheet.html"

build=false
profile=debug
for arg in "$@"; do
  case "$arg" in
    --build) build=true ;;
    --release) profile=release ;;
    --) ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      echo "contract-local: unknown argument: $arg" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [ "$build" = true ]; then
  if [ "$profile" = release ]; then
    cargo build --release --manifest-path "$manifest"
  else
    cargo build --manifest-path "$manifest"
  fi
fi

bin=${API_BIN:-"${CARGO_TARGET_DIR:-$repo_root/apps/api-rs/target}/$profile/yetracker-api"}
if [ ! -x "$bin" ]; then
  echo "contract-local: no API binary at $bin (build it with --build)" >&2
  exit 1
fi
if [ -n "$(find "$repo_root/apps/api-rs/src" "$manifest" "$repo_root/apps/api-rs/Cargo.lock" -newer "$bin" -print 2>/dev/null | head -n 1)" ]; then
  echo "contract-local: warning: $bin is older than the API sources (pass --build to rebuild)" >&2
fi

timeout=${CONTRACT_TIMEOUT:-60}
case "${CONTRACT_PORT:-0}${timeout}" in
  *[!0-9]*)
    echo "contract-local: CONTRACT_PORT and CONTRACT_TIMEOUT must be whole numbers" >&2
    exit 2
    ;;
esac
# A free port (CONTRACT_PORT is checked too): a busy one would point the tests at some other server.
port=$(node -e '
  const server = require("node:net").createServer();
  server.on("error", (error) => {
    console.error("contract-local: port " + process.argv[1] + " is not available (" + error.code + ")");
    process.exit(1);
  });
  server.listen(Number(process.argv[1]), "127.0.0.1", () => {
    console.log(server.address().port);
    server.close();
  });
' "${CONTRACT_PORT:-0}")
base_url="http://127.0.0.1:$port"
tmp=$(mktemp -d "${TMPDIR:-/tmp}/yetracker-contract.XXXXXX")
log="$tmp/api.log"
server_pid=

# shellcheck disable=SC2329 # invoked by the EXIT trap
cleanup() {
  local status=$?
  set +e
  if [ -n "$server_pid" ] && kill -0 "$server_pid" 2>/dev/null; then
    kill "$server_pid" 2>/dev/null
    # Graceful shutdown drains open connections for at most 10 s.
    for _ in $(seq 1 150); do
      kill -0 "$server_pid" 2>/dev/null || break
      sleep 0.1
    done
    kill -9 "$server_pid" 2>/dev/null
  fi
  [ -n "$server_pid" ] && wait "$server_pid" 2>/dev/null
  rm -rf "$tmp"
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

show_log() {
  echo "---- API log ($log, last 50 lines) ----" >&2
  tail -n 50 "$log" >&2
  echo "---- end of API log ----" >&2
}

# Every setting that affects the results is explicit: the API also reads the repo-root `.env` for unset variables.
env \
  API_HOST=127.0.0.1 \
  API_PORT="$port" \
  STORAGE_DIR="$tmp/storage" \
  SONGS_DIR="$tmp/storage/songs" \
  CATALOG_SHEET_FILE="$fixture" \
  IMPORT_FORCE=false \
  SYNC_ON_START=true \
  SYNC_INTERVAL_MINUTES=0 \
  DOWNLOADS_ENABLED=false \
  "$bin" >"$log" 2>&1 &
server_pid=$!

# Prints `ok <eras> <songs>`, `failed` or `pending`, from `/status`.
import_state() {
  node -e '
    fetch(process.argv[1] + "/status", { signal: AbortSignal.timeout(2000) })
      .then((res) => (res.ok ? res.json() : {}))
      .then((status) => {
        if (status.lastImportOk === true) console.log("ok " + status.eras + " " + status.songs);
        else console.log(status.lastImportOk === false ? "failed" : "pending");
      })
      .catch(() => console.log("pending"));
  ' "$base_url"
}

echo "contract-local: starting $bin on $base_url (storage: $tmp/storage)"
deadline=$((SECONDS + timeout))
while :; do
  read -r state eras songs <<<"$(import_state)"
  # Checked after the poll, so an answer only counts while this server is running.
  if ! kill -0 "$server_pid" 2>/dev/null; then
    echo "contract-local: the API exited before the fixture import finished" >&2
    show_log
    exit 1
  fi
  [ "$state" = ok ] && break
  if [ "$state" = failed ]; then
    echo "contract-local: the fixture import failed" >&2
    show_log
    exit 1
  fi
  if [ "$SECONDS" -ge "$deadline" ]; then
    echo "contract-local: timed out after ${timeout}s waiting for the fixture import" >&2
    show_log
    exit 1
  fi
  sleep 0.5
done
echo "contract-local: fixture imported ($eras eras, $songs songs); running the contract suite"

cd "$repo_root"
set +e
API_BASE_URL="$base_url" pnpm test:contract
status=$?
set -e
if [ "$status" -ne 0 ]; then
  show_log
fi
exit "$status"
