#!/usr/bin/env bash
# 20 MiB A/B pilot: initial version + five shared 4 KiB random patches.
# Run as oslab on openEuler. Does not run GC or delete existing data.
set -euo pipefail
export LC_ALL=C
unset MYVCS_DEBUG

case_dir="$HOME/os-lab/benchmarks/ab20-pilot01"
patch_dir="$HOME/os-lab/benchmarks/random20-pilot01/input"
results="$HOME/os-lab/repro-results/ab20-pilot01"
myvcs_bin="$HOME/os-lab/myvcs/target/release/myvcs"

fail() { printf 'STOP: %s\n' "$*" >&2; exit 1; }

for tool in python3 git dd cmp sha256sum stat tee; do
    command -v "$tool" >/dev/null || fail "Missing command: $tool"
done
[[ -x "$myvcs_bin" ]] || fail "Missing executable: $myvcs_bin"
[[ -x /usr/bin/time ]] || fail 'Missing /usr/bin/time'
[[ ! -e "$results" ]] || fail "Results already exist; do not rerun: $results"
[[ ! -e "$case_dir/myvcs/.myvcs" ]] || fail 'MyVCS repository already initialized'
[[ ! -e "$case_dir/git/.git" ]] || fail 'Git repository already initialized'

for relative in input/data.bin myvcs/data.bin git/data.bin; do
    path="$case_dir/$relative"
    [[ -f "$path" ]] || fail "Missing input: $path"
    [[ $(stat -c %s "$path") == 20971520 ]] || fail "Wrong input size: $path"
done
cmp "$case_dir/input/data.bin" "$case_dir/myvcs/data.bin"
cmp "$case_dir/input/data.bin" "$case_dir/git/data.bin"
python3 - "$case_dir/input/data.bin" <<'PY'
import sys
from pathlib import Path
half = 10 * 1024 * 1024
with Path(sys.argv[1]).open('rb') as f:
    if f.read(half) != b'A' * half or f.read(half) != b'B' * half or f.read(1):
        raise SystemExit('STOP: expected 10 MiB of A followed by 10 MiB of B')
PY

for n in 01 02 03 04 05; do
    patch="$patch_dir/patch-$n.bin"
    [[ -f "$patch" ]] || fail "Missing saved random patch: $patch"
    [[ $(stat -c %s "$patch") == 4096 ]] || fail "Wrong patch size: $patch"
    [[ ! -e "$case_dir/input/patch-$n.bin" ]] || fail "Patch already copied: $n"
done

mkdir -p "$(dirname "$results")"
mkdir "$results"
cp -- "${BASH_SOURCE[0]}" "$results/runner.sh"
for n in 01 02 03 04 05; do
    cp -- "$patch_dir/patch-$n.bin" "$case_dir/input/patch-$n.bin"
done
{
    date -Is
    uname -r
    git --version
    printf 'case=ab20-pilot01 bytes=20971520 versions=6 patch_bytes=4096\n'
    printf 'patch_offsets_MiB=1,2,3,4,5 patch_source=random20-pilot01\n'
    printf 'execution_order=myvcs_then_git cache_control=none timer=GNU_time\n'
    sha256sum "$myvcs_bin" "$case_dir/input/data.bin" "$case_dir/input"/patch-*.bin
} > "$results/manifest.txt"

cd "$case_dir/myvcs"
"$myvcs_bin" init > "$results/init-myvcs.log" 2>&1
cd "$case_dir/git"
git init -b main > "$results/init-git.log" 2>&1
git config --local user.name 'OS Lab'
git config --local user.email 'oslab@example.invalid'
git config --local commit.gpgsign false
git config --local gc.auto 0
git config --local --list > "$results/git-config.txt"

measure() {
    local tool="$1" phase="$2"
    shift 2
    local stem="$results/$phase-$tool"
    if /usr/bin/time -o "$stem.txt" \
        -f "tool=$tool phase=$phase elapsed_s=%e max_rss_KiB=%M exit=%x" \
        "$@" > "$stem.log" 2>&1; then
        cat "$stem.log"
        cat "$stem.txt" | tee -a "$results/timings.txt"
    else
        cat "$stem.log" >&2
        cat "$stem.txt" >&2
        fail "Failed: $tool $phase. Preserve this directory and report the error."
    fi
}

for n in 0 1 2 3 4 5; do
    if (( n == 0 )); then
        phase=initial
        message='Initial dataset'
    else
        printf -v label '%02d' "$n"
        phase="update$label"
        message="Update dataset $label"
        for tool in myvcs git; do
            dd if="$case_dir/input/patch-$label.bin" of="$case_dir/$tool/data.bin" \
                bs=4096 seek=$((n * 256)) count=1 conv=notrunc status=none
        done
    fi
    printf '\n=== %s ===\n' "$phase"
    cmp "$case_dir/myvcs/data.bin" "$case_dir/git/data.bin"
    sha256sum "$case_dir/myvcs/data.bin" "$case_dir/git/data.bin" \
        > "$results/$phase-data-sha256.txt"
    cd "$case_dir/myvcs"
    measure myvcs "${phase}_add" "$myvcs_bin" add data.bin
    measure myvcs "${phase}_commit" "$myvcs_bin" commit -m "$message"
    cd "$case_dir/git"
    measure git "${phase}_add" git add data.bin
    measure git "${phase}_commit" git commit -m "$message"
done

cd "$case_dir/myvcs"
"$myvcs_bin" status | tee "$results/final-status-myvcs.txt"
cd "$case_dir/git"
git status | tee "$results/final-status-git.txt"
git rev-list --count HEAD | tee "$results/git-commit-count.txt"
[[ $(git rev-list --count HEAD) == 6 ]] || fail 'Expected six Git commits'
printf '\nPilot commits completed. GC has NOT run. Results: %s\n' "$results"
