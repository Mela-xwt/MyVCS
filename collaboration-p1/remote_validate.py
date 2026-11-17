"""Isolated openEuler P1 validation. Password is interactive and never persisted."""
import argparse,getpass,hashlib,json,shlex,sys,tarfile
from pathlib import Path
import paramiko
root=Path(__file__).resolve().parent.parent;out=root/'collaboration-p1'
parser=argparse.ArgumentParser();parser.add_argument('--resume',action='store_true');parser.add_argument('--refresh',action='store_true');args=parser.parse_args()
with tarfile.open(out/'candidate-source.tar.gz','w:gz') as t:
 for f in [root/'Cargo.toml',root/'Cargo.lock',*sorted((root/'src').rglob('*.rs')),*sorted((root/'tests').rglob('*.rs')),*sorted((root/'collaboration-stage3').glob('*.py')),out/'benchmark.py',out/'compatibility.py']:
  t.add(f,arcname=f.relative_to(root).as_posix())
c=paramiko.SSHClient();c.load_host_keys(str(root/'collaboration-stage2/known_hosts'))
c.connect('192.168.220.129',username='oslab',password=getpass.getpass('SSH password: '),allow_agent=False,look_for_keys=False,timeout=15)
def run(command,log):
 _,stdout,_=c.exec_command('{ '+command+'; } 2>&1',timeout=30)
 with (out/log).open('w',encoding='utf-8') as f:
  for line in stdout:f.write(line);f.flush();print(line,end='',flush=True)
 return stdout.channel.recv_exit_status()
try:
 if args.resume or args.refresh: remote=(out/'server-directory.txt').read_text().strip()
 else:
  _,o,_=c.exec_command('mktemp -d /home/oslab/os-lab/myvcs-p1.XXXXXX');remote=o.read().decode().strip()
 assert remote.startswith('/home/oslab/os-lab/myvcs-p1.')
 (out/'server-directory.txt').write_text(remote+'\n');s=c.open_sftp();s.put(str(out/'candidate-source.tar.gz'),remote+'/source.tar.gz');s.put(str(out/'baseline-source.tar.gz'),remote+'/baseline.tar.gz');q=shlex.quote(remote)
 cmd=f'cd {q} && tar -xzf source.tar.gz && mkdir -p baseline && tar -xzf baseline.tar.gz -C baseline && export PATH="$HOME/.cargo/bin:$PATH" && uname -a && rustc --version && cargo --version && df -h . && cargo test --locked --offline && cargo build --release --locked --offline && cd baseline && cargo build --release --locked --offline'
 if not args.resume:
  code=run(cmd,'server-build-tests.log');(out/'server-build-exit-code.txt').write_text(str(code));assert code==0
  code=run(f'cd {q} && python3 collaboration-stage3/ssh_acceptance.py --bin {q}/target/release/myvcs','server-ssh.log');assert code==0
 else:
  s.put(str(out/'compatibility.py'),remote+'/collaboration-p1/compatibility.py')
  s.put(str(out/'benchmark.py'),remote+'/collaboration-p1/benchmark.py')
 code=run(f'cd {q} && python3 collaboration-p1/compatibility.py --old {q}/baseline/target/release/myvcs --new {q}/target/release/myvcs --quiescent','server-p1-compatibility.log');assert code==0
 code=run(f'cd {q} && ./target/release/myvcs doctor --json && python3 collaboration-p1/benchmark.py --old {q}/baseline/target/release/myvcs --new {q}/target/release/myvcs --out {q}/measurements && sha256sum source.tar.gz baseline.tar.gz target/release/myvcs baseline/target/release/myvcs','server-benchmark.log');assert code==0
 (out/'measurements').mkdir(exist_ok=True)
 for n in s.listdir(remote+'/measurements'):
  if n.endswith('.json'):s.get(remote+'/measurements/'+n,str(out/'measurements'/n))
 print('PASS server P1 build, tests, SSH, compatibility, paired benchmark')
finally:c.close()
