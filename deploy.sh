#!/usr/bin/env bash
# Jedyna droga wdrożenia after15: buduje z TEGO katalogu i instaluje do
# ~/.cargo/bin/after15, czyli tam, gdzie widzi go PATH w zsh, serwis
# after15-web i symlink /home/jarek/after15 dla paska Claude.
# Nie używaj `cargo build --release` do wdrożenia: jego wynik ląduje we
# wspólnym ~/.cargo/shared-target, do którego pisze każdy worktree.
set -euo pipefail
cd "$(dirname "$0")"

cargo install --path . --locked
ln -sfn "$HOME/.cargo/bin/after15" "$HOME/after15"

export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
systemctl --user restart after15-web
sleep 1
systemctl --user is-active after15-web
echo "after15 -> $(readlink -f "$HOME/.cargo/bin/after15") ($(date -r "$HOME/.cargo/bin/after15" +%H:%M:%S))"
