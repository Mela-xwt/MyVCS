#!/bin/bash
set -eu
export PATH="$HOME/.cargo/bin:$PATH"
root=/mnt/d/DOWNLOAD/OperatingSystems/group_work
lintdir=$(mktemp -d /tmp/myvcs-p1-lint.XXXXXX)
tar -xzf "$root/collaboration-p1/baseline-source.tar.gz" -C "$lintdir"
cd "$lintdir"
CARGO_TARGET_DIR=/tmp/myvcs-p1-baseline-target cargo clippy --locked --offline --all-targets > "$root/collaboration-p1/baseline-clippy.log" 2>&1
