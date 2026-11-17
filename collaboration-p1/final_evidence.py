"""Collect P1 CLI demonstration and verify the tested server source snapshot."""
import getpass
import json
from pathlib import Path
import shlex
import paramiko

root = Path(__file__).resolve().parent.parent
out = root / 'collaboration-p1'
remote = (out / 'server-directory.txt').read_text().strip()
assert remote.startswith('/home/oslab/os-lab/myvcs-p1.')
manifest = json.loads((out / 'source-manifest.json').read_text())
c = paramiko.SSHClient()
c.load_host_keys(str(root / 'collaboration-stage2/known_hosts'))
c.connect('192.168.220.129', username='oslab', password=getpass.getpass('SSH password: '), allow_agent=False, look_for_keys=False)
try:
    s = c.open_sftp()
    for name in ['demo.py', 'source-manifest.json']:
        s.put(str(out / name), remote + '/collaboration-p1/' + name)
    checker = 'import json,hashlib; from pathlib import Path; m=json.loads(Path("collaboration-p1/source-manifest.json").read_text()); assert all(hashlib.sha256(Path(p).read_bytes()).hexdigest()==v for p,v in m["files_sha256"].items()); print("PASS all server source/test SHA256 match implementation commit",m["implementation_commit"])'
    q = shlex.quote(remote)
    command = f'cd {q} && python3 -c {shlex.quote(checker)} && python3 collaboration-p1/demo.py --bin {q}/target/release/myvcs'
    _, o, _ = c.exec_command('{ ' + command + '; } 2>&1', timeout=30)
    with (out / 'server-demo.log').open('w', encoding='utf-8') as f:
        for line in o:
            f.write(line); f.flush(); print(line, end='', flush=True)
    assert o.channel.recv_exit_status() == 0
    (out / 'bin').mkdir(exist_ok=True)
    s.get(remote + '/target/release/myvcs', str(out / 'bin/myvcs-openEuler-x86_64'))
finally:
    c.close()
