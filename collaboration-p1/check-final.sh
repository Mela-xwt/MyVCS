#!/bin/bash
set -eu
export PATH="$HOME/.cargo/bin:$PATH"
cd /mnt/d/DOWNLOAD/OperatingSystems/group_work
cargo fmt --check > collaboration-p1/format.log 2>&1
cargo test --locked --offline > collaboration-p1/all-tests.log 2>&1
cargo clippy --locked --offline --all-targets > collaboration-p1/clippy-warnings.log 2>&1
