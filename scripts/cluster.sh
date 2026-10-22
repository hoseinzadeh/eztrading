#!/bin/bash
# Start or stop a local cluster (srvman starts datasrv, codesrv and jobsrv over SSH).
#   scripts/cluster.sh up [SECONDS_TO_WAIT]   (default 15)
#   scripts/cluster.sh down
# Needs a built workspace (cargo build --workspace), system.toml in the repository root, MySQL/MariaDB
# with db/schema.sql loaded, `ssh localhost` working without a password, and scripts that read
# TWELVEDATA_API_URL / TWELVEDATA_WS_URL from the *ssh* environment if you use tools/fake_twelvedata.py
# (see docs/cluster.md).
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
case "${1:-up}" in
  down)
    pkill -x srvman; pkill -x datasrv; pkill -x jobsrv; pkill -x codesrv
    ;;
  up)
    "$0" down; sleep 1
    mkdir -p "$HOME/.eztrading"; rm -f "$HOME/.eztrading/"*.{out,err,pid} 2>/dev/null
    cd "$ROOT" && (nohup ./target/debug/srvman > "$HOME/.eztrading/srvman.log" 2>&1 &)
    sleep "${2:-15}"
    echo "srvman log: $HOME/.eztrading/srvman.log; server logs: $HOME/.eztrading/<name>.out"
    ps -eo pid,comm | grep -E "srvman|datasrv|jobsrv|codesrv"
    ;;
  *) echo "usage: $0 up|down"; exit 2 ;;
esac
