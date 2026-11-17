"""Repeatable CLI demonstration in a new temporary repository; prints real output."""
import argparse
import hashlib
import json
from pathlib import Path
import shlex
import subprocess
import tempfile

p = argparse.ArgumentParser()
p.add_argument('--bin', required=True)
a = p.parse_args()
binary = str(Path(a.bin).resolve())
root = Path(tempfile.mkdtemp(prefix='myvcs-p1-demo-'))

def run(*args, ok=True):
    print('$', shlex.join([binary, '--repo', str(root), *args]), flush=True)
    r = subprocess.run([binary, '--repo', str(root), *args], capture_output=True, text=True)
    print(r.stdout + r.stderr, end='', flush=True)
    print('exit =', r.returncode, flush=True)
    assert (r.returncode == 0) == ok
    return r.stdout

def write(name, text):
    print('WRITE', name, '=', repr(text), flush=True)
    (root / name).write_text(text)

print('Workspace:', root, 'Binary SHA256:', hashlib.sha256(Path(binary).read_bytes()).hexdigest())
subprocess.run([binary, 'init', '--empty', str(root)], check=True)
write('f.txt', 'base\n')
run('add', 'f.txt'); run('commit', '-m', 'base')
head = (root / '.myvcs/refs/heads/main').read_bytes()
print('\nF01 shared ignore rules')
write('.myvcsignore', '*.log\n!important.log\ntarget/\n')
write('debug.log', 'ignored\n'); write('important.log', 'included\n')
run('status'); run('check-ignore', 'debug.log', 'important.log')
run('add', '.'); run('commit', '-m', 'ignore setup')
head = (root / '.myvcs/refs/heads/main').read_bytes()
print('\nF02 staged versus working bytes and single-file restore')
write('f.txt', 'staged\n'); run('add', 'f.txt'); write('f.txt', 'working\n')
d = run('diff', '--staged', 'f.txt'); assert '+staged' in d and '+working' not in d
run('restore', 'f.txt'); assert (root / 'f.txt').read_text() == 'base\n'
run('diff', '--staged'); run('restore', '--staged', 'f.txt'); assert not run('diff', '--staged')
assert (root / '.myvcs/refs/heads/main').read_bytes() == head
print('PASS single-file restore preserved HEAD and unrelated files')
print('\nF03 independent index/worktree, untracked files, GC and pop safety')
write('f.txt', 'saved index\n'); run('add', 'f.txt'); write('f.txt', 'saved work\n'); write('notes.txt', 'untracked\n')
index = (root / '.myvcs/index.json').read_bytes()
run('stash', 'push', '--include-untracked', '-m', 'P1 demonstration')
run('stash', 'list'); run('stash', 'show'); run('gc'); run('verify')
write('f.txt', 'new local edits\n'); run('stash', 'pop', ok=False)
assert (root / 'f.txt').read_text() == 'new local edits\n'
run('stash', 'list'); run('restore', 'f.txt'); run('stash', 'pop')
assert (root / 'f.txt').read_text() == 'saved work\n'
assert (root / 'notes.txt').read_text() == 'untracked\n'
assert (root / '.myvcs/index.json').read_bytes() == index
assert (root / 'debug.log').read_text() == 'ignored\n'
run('diff', '--staged'); assert not run('stash', 'list')
print('PASS worktree, index and untracked bytes restored after GC; ignored file untouched')
print('\nF04/F05 bounded delta and honest diagnostics (full comparison in benchmark.py)')
run('doctor'); json.loads(run('doctor', '--json')); json.loads(run('stats', '--json')); run('verify')
print('PASS P1 CLI demonstration; retained workspace:', root)
