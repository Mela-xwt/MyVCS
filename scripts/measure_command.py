#!/usr/bin/env python3
"""Linux benchmark timer. One invocation measures one external command.

Wall time includes launching GNU time and its child, but excludes Python startup
and writing the result JSON. GNU time supplies the command's peak RSS in KiB.
Command stdout/stderr are combined in a sibling .log file, outside the console.
Existing results/logs are never overwritten. A failed command remains a failure.
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--label', required=True)
    parser.add_argument('--cwd', type=Path, default=Path.cwd())
    parser.add_argument('command', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command
    if command and command[0] == '--':
        command = command[1:]
    if not command:
        parser.error('Provide a command after --')
    if not sys.platform.startswith('linux'):
        parser.error('Run this timer on Linux; RSS units here assume Linux GNU time')
    if not os.access('/usr/bin/time', os.X_OK):
        parser.error('Missing executable /usr/bin/time')
    cwd = args.cwd.resolve()
    if not cwd.is_dir():
        parser.error('Working directory does not exist: ' + str(cwd))
    executable = command[0]
    if '/' in executable:
        resolved = Path(executable)
        if not resolved.is_absolute():
            resolved = cwd / resolved
        executable = str(resolved.resolve())
    else:
        executable = shutil.which(executable)
    if not executable or not os.access(executable, os.X_OK):
        parser.error('Command executable not found: ' + command[0])
    command = [executable] + command[1:]

    output = args.output.resolve()
    if output.suffix != '.json':
        parser.error('--output must end with .json')
    log_path = output.with_suffix('.log')
    if output.exists() or log_path.exists():
        parser.error('Result or log already exists; choose a new output path')
    output.parent.mkdir(parents=True, exist_ok=True)

    record = {
        'schema_version': 1,
        'label': args.label,
        'command': command,
        'cwd': str(cwd),
        'wall_timer': 'perf_counter_ns around GNU time and command',
        'rss_source': 'GNU time %M (KiB on Linux)',
        'log': str(log_path),
        'status': 'incomplete',
    }
    # Exclusive creation prevents accidental replacement of earlier measurements.
    with output.open('x', encoding='utf-8') as result_file:
        json.dump(record, result_file, indent=2)
        result_file.flush()
        with log_path.open('x', encoding='utf-8') as command_log:
            with tempfile.TemporaryDirectory(prefix='myvcs-timer-') as tmp:
                resource_file = Path(tmp) / 'resource.txt'
                argv = ['/usr/bin/time', '-o', str(resource_file),
                        '-f', 'MEASURE_RSS_KIB=%M', '--'] + command
                try:
                    started = time.perf_counter_ns()
                    process = subprocess.run(argv, cwd=str(cwd),
                                             stdout=command_log,
                                             stderr=subprocess.STDOUT)
                    elapsed_ns = time.perf_counter_ns() - started
                    record['elapsed_ms'] = round(elapsed_ns / 1_000_000, 6)
                    record['exit_code'] = process.returncode
                    lines = resource_file.read_text().splitlines()
                    rss = [line.split('=', 1)[1] for line in lines
                           if line.startswith('MEASURE_RSS_KIB=')]
                    if len(rss) != 1:
                        raise ValueError('GNU time did not return one RSS record')
                    record['max_rss_KiB'] = int(rss[0])
                    record['status'] = ('ok' if process.returncode == 0
                                        else 'command_failed')
                except Exception as error:
                    record['status'] = 'measurement_error'
                    record['error'] = str(error)
        result_file.seek(0)
        json.dump(record, result_file, indent=2, ensure_ascii=False)
        result_file.write('\n')
        result_file.truncate()

    print(json.dumps(record, indent=2, ensure_ascii=False))
    if record['status'] == 'ok':
        return 0
    return record.get('exit_code', 1) or 1


if __name__ == '__main__':
    sys.exit(main())
