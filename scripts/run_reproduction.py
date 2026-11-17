#!/usr/bin/env python3
"""Run the four MyVCS/Git cases on Linux, using preserved pilot inputs.

Use --check first. --rounds 1 is an integration check, not a 20-round result.
Only successfully verified temporary trial repositories created by this run are
deleted. Inputs, logs, measurements, and any failed trial are retained.
"""
import argparse
import csv
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import statistics
import subprocess
import sys
import time

MIB = 1024 * 1024
CASES = {
    'random20': (20, 5, 4096, 'dbc9463a11f44fe91fba0379211f79c076cf54897496acb3d0c6926de2ddf7c7'),
    'ab20': (20, 5, 4096, '51a179c6f2c2566e5c15489825fa40d5767ff18abfeb1d031ada8dd8c05d7a25'),
    'random128': (128, 2, 65536, '7129de70fbbd535610bf7c8b1e13dbc55130669b36fd59dab868c8cf3c22cd9d'),
    'ab128': (128, 2, 65536, '76081402138eee7cf2af1ba2315aef73950cf3a843da3d1c7bdab61c7ed7c879'),
}
BASELINE_BINARY = '80477e397c8c7c6ab5452a9135e23a1e0a98f77e900fc33a2da95f6872507e36'


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def digest(path):
    value = hashlib.sha256()
    with Path(path).open('rb') as f:
        for chunk in iter(lambda: f.read(MIB), b''):
            value.update(chunk)
    return value.hexdigest()


def save_json(path, data):
    with Path(path).open('x', encoding='utf-8') as f:
        json.dump(data, f, indent=2, ensure_ascii=False)
        f.write('\n')


def snapshot():
    return {name: Path('/proc/' + name).read_text()
            for name in ('loadavg', 'meminfo', 'vmstat')}


def object_bytes(repo, tool):
    folder = repo / ('.myvcs' if tool == 'myvcs' else '.git') / 'objects'
    require(folder.is_dir(), 'Missing objects: ' + str(folder))
    return sum(p.stat().st_size for p in folder.rglob('*') if p.is_file())


def clean_trial(trial, work_root, run_name):
    # Reject symlinks and require an immediate child with this run's marker.
    require(not trial.is_symlink(), 'Refusing to clean a symlink')
    resolved = trial.resolve()
    require(resolved.parent == work_root.resolve(), 'Cleanup outside run directory')
    require(resolved.name.startswith('trial-'), 'Unexpected cleanup directory')
    require((resolved / '.trial-owner').read_text() == run_name,
            'Cleanup ownership marker mismatch')
    shutil.rmtree(resolved)


def run_plain(command, cwd, log, env):
    with log.open('x', encoding='utf-8') as f:
        p = subprocess.run(command, cwd=str(cwd), env=env,
                           stdout=f, stderr=subprocess.STDOUT)
    require(p.returncode == 0, 'Command failed; see ' + str(log))
    return log.read_text(encoding='utf-8', errors='replace')


def measure(timer, command, cwd, path, env):
    save_json(path.with_name(path.stem + '-system-before.json'), snapshot())
    p = subprocess.run([sys.executable, str(timer), '--output', str(path),
                        '--label', path.stem, '--cwd', str(cwd), '--'] + command,
                       env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                       universal_newlines=True)
    require(p.returncode == 0, 'Measurement failed: ' + str(path) + '\n' + p.stderr)
    record = json.loads(path.read_text())
    require(record['status'] == 'ok' and record['exit_code'] == 0,
            'Invalid measurement: ' + str(path))
    save_json(path.with_name(path.stem + '-system-after.json'), snapshot())
    return record


def check_inputs(lab, binary, timer):
    require(sys.platform.startswith('linux'), 'Run on Linux')
    require(timer.is_file(), 'Missing timer: ' + str(timer))
    require(os.access('/usr/bin/time', os.X_OK), 'Missing /usr/bin/time')
    require(shutil.which('git') is not None, 'Missing git')
    require(os.access(binary, os.X_OK), 'Missing MyVCS binary')
    require(digest(binary) == BASELINE_BINARY, 'MyVCS binary differs from pilot baseline')
    require(shutil.disk_usage(lab).free >= 4 * 1024**3, 'Less than 4 GiB free space')
    inputs = {}
    for case, (size, updates, patch_bytes, expected) in CASES.items():
        folder = lab / 'benchmarks' / (case + '-pilot01') / 'input'
        data = folder / 'data.bin'
        require(data.stat().st_size == size * MIB, 'Wrong base size: ' + case)
        require(digest(data) == expected, 'Base input changed: ' + case)
        inputs[case] = {'data.bin': expected}
        for n in range(1, updates + 1):
            name = 'patch-%02d.bin' % n
            require((folder / name).stat().st_size == patch_bytes,
                    'Wrong patch size: ' + str(folder / name))
            inputs[case][name] = digest(folder / name)
    for size in (20, 128):
        random_case, ab_case = 'random' + str(size), 'ab' + str(size)
        for name in inputs[random_case]:
            if name != 'data.bin':
                require(inputs[random_case][name] == inputs[ab_case][name],
                        'Random and A/B patches differ: ' + name)
    return inputs


def trial_run(case, round_no, first, trial, log_dir, fixture, binary, timer, env):
    size, updates, _, _ = CASES[case]
    order = [first, 'git' if first == 'myvcs' else 'myvcs']
    executables = {'myvcs': str(binary), 'git': shutil.which('git')}
    repos = {tool: trial / tool for tool in order}
    records = {tool: {} for tool in order}
    commits = {tool: [] for tool in order}
    expected_hashes = []
    for tool in order:
        repo = repos[tool]
        repo.mkdir()
        shutil.copyfile(fixture / 'data.bin', repo / 'data.bin')
        (log_dir / tool).mkdir()
        init = ([executables[tool], 'init'] if tool == 'myvcs' else
                [executables[tool], 'init', '--template=', '-b', 'main'])
        run_plain(init, repo, log_dir / tool / 'init.log', env)
        if tool == 'git':
            settings = {'user.name': 'OS Lab', 'user.email': 'oslab@example.invalid',
                        'commit.gpgsign': 'false', 'gc.auto': '0'}
            for key, value in settings.items():
                run_plain([executables[tool], 'config', '--local', key, value], repo,
                          log_dir / tool / ('config-' + key + '.log'), env)
            run_plain([executables[tool], 'config', '--list', '--show-origin'], repo,
                      log_dir / tool / 'effective-config.log', env)

    for n in range(updates + 1):
        phase = 'initial' if n == 0 else 'update%02d' % n
        if n:
            patch = (fixture / ('patch-%02d.bin' % n)).read_bytes()
            for tool in order:
                with (repos[tool] / 'data.bin').open('r+b') as f:
                    f.seek(n * MIB)
                    f.write(patch)
        hashes = {tool: digest(repos[tool] / 'data.bin') for tool in order}
        require(len(set(hashes.values())) == 1, 'Tool inputs differ at ' + phase)
        for tool in order:
            require((repos[tool] / 'data.bin').stat().st_size == size * MIB,
                    'Data size changed')
        expected_hashes.append(next(iter(hashes.values())))
        save_json(log_dir / (phase + '-input-hashes.json'), hashes)
        for tool in order:
            for operation, args in [('add', ['add', 'data.bin']),
                                    ('commit', ['commit', '-m', 'Dataset ' + phase])]:
                key = phase + '-' + operation
                records[tool][key] = measure(timer, [executables[tool]] + args,
                                             repos[tool], log_dir / tool / (key + '.json'), env)
            if tool == 'myvcs':
                oid = (repos[tool] / '.myvcs/refs/heads/main').read_text().strip()
            else:
                oid = run_plain([executables[tool], 'rev-parse', 'HEAD'], repos[tool],
                                log_dir / tool / (phase + '-oid.log'), env).strip()
            commits[tool].append(oid)
        print('  ' + phase + ' committed by both tools', flush=True)

    before = {tool: object_bytes(repos[tool], tool) for tool in order}
    save_json(log_dir / 'storage-before-gc.json', before)
    for tool in order:
        records[tool]['gc'] = measure(timer, [executables[tool], 'gc'], repos[tool],
                                      log_dir / tool / 'gc.json', env)
        if tool == 'myvcs':
            text = (log_dir / tool / 'gc.log').read_text()
            require('deep-delta optimization skipped' not in text,
                    'GC skipped delta optimization due to load; trial retained for inspection')
        args = ['verify'] if tool == 'myvcs' else ['fsck', '--full']
        records[tool]['verify'] = measure(timer, [executables[tool]] + args, repos[tool],
                                          log_dir / tool / 'verify.json', env)
    after = {tool: object_bytes(repos[tool], tool) for tool in order}
    save_json(log_dir / 'storage-after-gc.json', after)

    # Independently check every restored data version after GC, outside timings.
    for tool in order:
        restored = []
        for n, oid in enumerate(commits[tool]):
            run_plain([executables[tool], 'checkout', oid], repos[tool],
                      log_dir / tool / ('restore-%02d.log' % n), env)
            actual = digest(repos[tool] / 'data.bin')
            require(actual == expected_hashes[n], 'Restored data mismatch: ' + tool)
            restored.append({'commit': oid, 'sha256': actual})
        run_plain([executables[tool], 'checkout', 'main'], repos[tool],
                  log_dir / tool / 'return-main.log', env)
        run_plain([executables[tool], 'status'], repos[tool],
                  log_dir / tool / 'final-status.log', env)
        save_json(log_dir / tool / 'restored-versions.json', restored)

    rows = []
    for tool in order:
        r = records[tool]
        update_add = [r['update%02d-add' % n]['elapsed_ms'] for n in range(1, updates + 1)]
        update_commit = [r['update%02d-commit' % n]['elapsed_ms'] for n in range(1, updates + 1)]
        row = {'case': case, 'round': round_no, 'tool': tool, 'first_tool': first,
               'initial_add_ms': r['initial-add']['elapsed_ms'],
               'initial_commit_ms': r['initial-commit']['elapsed_ms'],
               'initial_total_ms': r['initial-add']['elapsed_ms'] + r['initial-commit']['elapsed_ms'],
               'update_add_mean_ms': statistics.mean(update_add),
               'update_commit_mean_ms': statistics.mean(update_commit),
               'update_total_mean_ms': statistics.mean(a + b for a, b in zip(update_add, update_commit)),
               'submission_peak_rss_KiB': max(v['max_rss_KiB'] for k, v in r.items()
                                             if k.endswith('-add') or k.endswith('-commit')),
               'gc_ms': r['gc']['elapsed_ms'], 'gc_peak_rss_KiB': r['gc']['max_rss_KiB'],
               'verify_ms': r['verify']['elapsed_ms'],
               'verify_peak_rss_KiB': r['verify']['max_rss_KiB'],
               'object_bytes_before_gc': before[tool], 'object_bytes_after_gc': after[tool],
               'saving_vs_full_versions_pct': 100 * (1 - after[tool] / ((updates + 1) * size * MIB)),
               'restored_versions_checked': updates + 1}
        rows.append(row)
    save_json(log_dir / 'summary.json', rows)
    return rows


def aggregate(rows):
    groups = {}
    excluded = {'case', 'round', 'tool', 'first_tool', 'restored_versions_checked'}
    for case in CASES:
        groups[case] = {}
        for tool in ('myvcs', 'git'):
            selected = [r for r in rows if r['case'] == case and r['tool'] == tool]
            metrics = {}
            for key in selected[0]:
                if key in excluded:
                    continue
                values = [r[key] for r in selected]
                metrics[key] = {'mean': statistics.mean(values),
                                'median': statistics.median(values),
                                'stdev': statistics.stdev(values) if len(values) > 1 else None,
                                'min': min(values), 'max': max(values)}
            groups[case][tool] = {'rounds': len(selected), 'metrics': metrics}
    return groups


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true')
    parser.add_argument('--run-name', default='formal-smoke01')
    parser.add_argument('--rounds', type=int, default=1)
    args = parser.parse_args()
    require(re.fullmatch(r'formal-[a-z0-9][a-z0-9-]{0,48}', args.run_name),
            'Run name must start with formal- and contain lowercase letters/digits/hyphens')
    require(1 <= args.rounds <= 20, 'Choose 1 to 20 rounds')
    lab = Path.home() / 'os-lab'
    binary = lab / 'myvcs/target/release/myvcs'
    timer = Path(__file__).resolve().with_name('measure_command.py')
    inputs = check_inputs(lab, binary, timer)
    results = lab / 'repro-results' / args.run_name
    work = lab / 'benchmarks' / args.run_name
    require(not results.exists() and not work.exists(), 'Run name already exists; no files overwritten')
    if args.check:
        print('CHECK OK: baseline binary, four datasets, shared patches, tools, free space, new paths.')
        print('No experiments run and no files changed.')
        return

    results.mkdir()
    work.mkdir()
    env = {k: v for k, v in os.environ.items()
           if not k.startswith('MYVCS_') and not k.startswith('GIT_')}
    env.update({'LC_ALL': 'C', 'GIT_CONFIG_NOSYSTEM': '1', 'GIT_CONFIG_GLOBAL': '/dev/null'})
    manifest = {'run_name': args.run_name, 'rounds': args.rounds,
                'started_local': time.strftime('%Y-%m-%dT%H:%M:%S%z'),
                'inputs': inputs, 'binary_sha256': digest(binary),
                'python': sys.version, 'uname': list(os.uname()),
                'os_release': Path('/etc/os-release').read_text(),
                'cpuinfo': Path('/proc/cpuinfo').read_text(),
                'cpu_affinity': sorted(os.sched_getaffinity(0)),
                'initial_system': snapshot(),
                'method': {
                    'data': 'Fixed preserved input and patches reused across fresh repositories',
                    'cache': 'No cache dropping; copying and hashing warms input pages',
                    'order': 'Odd rounds MyVCS first; even rounds Git first; case order rotates',
                    'increment_offsets_bytes': 'n * 1048576 for n=1..update_count',
                    'git': 'No global/system config or init templates; local identity; signing off; gc.auto=0',
                    'myvcs': 'All inherited MYVCS_* overrides removed; default policies retained',
                    'gc_skip': 'Stop and retain evidence if load-aware delta GC is skipped; no silent retry',
                    'output': 'Command stdout/stderr redirected to per-command logs',
                    'memory': 'GNU time peak RSS; submission, GC, and verification reported separately',
                    'wall_time': 'perf_counter_ns around GNU time + command; do not subtract selfcheck timings',
                    'cleanup': 'Only completed and verified temporary trial repos removed; failures retained',
                }}
    save_json(results / 'manifest.json', manifest)
    for path in (Path(__file__).resolve(), timer):
        shutil.copyfile(path, results / path.name)
    for name in ('Cargo.toml', 'Cargo.lock'):
        shutil.copyfile(lab / 'myvcs' / name, results / name)
    source = lab / 'myvcs/src'
    shutil.copytree(source, results / 'src')
    run_plain(['git', '--version'], lab, results / 'git-version.log', env)
    run_plain(['rustc', '--version'], lab, results / 'rustc-version.log', env)
    fixtures = work / 'inputs'
    fixtures.mkdir()
    for case in CASES:
        destination = fixtures / case
        destination.mkdir()
        for name, expected in inputs[case].items():
            shutil.copyfile(lab / 'benchmarks' / (case + '-pilot01') / 'input' / name,
                            destination / name)
            require(digest(destination / name) == expected, 'Fixture copy mismatch')

    all_rows = []
    with (results / 'results.csv').open('x', newline='', encoding='utf-8') as f:
        writer = None
        for round_no in range(1, args.rounds + 1):
            cases = list(CASES)
            shift = (round_no - 1) % len(cases)
            cases = cases[shift:] + cases[:shift]
            first = 'myvcs' if round_no % 2 else 'git'
            for case in cases:
                require(shutil.disk_usage(lab).free >= 3 * 1024**3, 'Less than 3 GiB free space; stopped')
                name = 'trial-%02d-%s' % (round_no, case)
                trial = work / name
                trial.mkdir()
                (trial / '.trial-owner').write_text(args.run_name)
                log_dir = results / name
                log_dir.mkdir()
                print('\nRound %d/%d case=%s first=%s' % (round_no, args.rounds, case, first), flush=True)
                rows = trial_run(case, round_no, first, trial, log_dir, fixtures / case,
                                 binary, timer, env)
                if writer is None:
                    writer = csv.DictWriter(f, fieldnames=list(rows[0]))
                    writer.writeheader()
                writer.writerows(rows)
                f.flush()
                all_rows.extend(rows)
                clean_trial(trial, work, args.run_name)
                print('  PASS: measurements, integrity, every restored version; temporary repos removed.', flush=True)
    save_json(results / 'aggregate.json', aggregate(all_rows))
    save_json(results / 'completed.json', {'rounds': args.rounds, 'case_trials': args.rounds * 4,
                                          'tool_trials': len(all_rows), 'final_system': snapshot()})
    print('\nCOMPLETE: ' + str(results), flush=True)
    print('Results CSV: ' + str(results / 'results.csv'), flush=True)


if __name__ == '__main__':
    try:
        main()
    except (Exception, KeyboardInterrupt) as error:
        print('\nSTOP: %s\nExisting results and unfinished trial repositories were retained. '
              'Do not rerun under the same name; inspect the failure first.' % error, file=sys.stderr)
        sys.exit(1)
