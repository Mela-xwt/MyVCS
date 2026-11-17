"""Package complete P1 sources, support scripts and evidence; verify every ZIP member."""
import hashlib
import json
from pathlib import Path
import subprocess
import zipfile

root = Path(__file__).resolve().parent.parent
out = root/'collaboration-p1'
destination = out/'MyVCS-P1-source-and-evidence.zip'
files = {root/'Cargo.toml', root/'Cargo.lock', root/'README.md'}
for folder, suffixes in [('src',{'.rs'}), ('tests',{'.rs'}), ('docs',{'.md'}),
                         ('scripts',{'.py','.sh','.md'}),
                         ('collaboration-stage3',{'.py','.md','.log'})]:
    files.update(p for p in (root/folder).rglob('*') if p.is_file() and p.suffix in suffixes and '__pycache__' not in p.parts)
# Public host-key trust record; no credentials or private keys are included.
files.add(root/'collaboration-stage2/known_hosts')
for p in out.rglob('*'):
    if not p.is_file() or '__pycache__' in p.parts:
        continue
    if p.name in {'package-manifest.json','SHA256SUMS.txt'} or p.suffix in {'.zip','.bundle'}:
        continue
    if p.suffix in {'.py','.sh','.md','.json','.log','.txt','.gz'} or p.parent.name=='bin':
        files.add(p)
bundle = out/'MyVCS-P1-history.bundle'
subprocess.run(['git','bundle','create',str(bundle),'feature/p1-extensions'],cwd=root,check=True)
subprocess.run(['git','bundle','verify',str(bundle)],cwd=root,check=True,capture_output=True)
files.add(bundle)
source = json.loads((out/'source-manifest.json').read_text())
records = {p.relative_to(root).as_posix(): hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(files)}
for name, value in source['files_sha256'].items():
    assert records[name] == value
manifest = {'root':'MyVCS-P1','implementation_commit':source['implementation_commit'],
            'documentation_commit':subprocess.check_output(['git','rev-parse','HEAD'],cwd=root,text=True).strip(),
            'file_count':len(records),'sha256':records,
            'exclusions':['.git directory','build target directories','private SSH keys','passwords','unrelated coursework files','Python bytecode'],
            'binary_platform':'openEuler 22.03 LTS x86_64; rebuild for other platforms'}
(out/'package-manifest.json').write_text(json.dumps(manifest,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
files.add(out/'package-manifest.json')
with zipfile.ZipFile(destination,'w',compression=zipfile.ZIP_DEFLATED,compresslevel=9) as z:
    for p in sorted(files):
        z.write(p,'MyVCS-P1/'+p.relative_to(root).as_posix())
with zipfile.ZipFile(destination) as z:
    assert z.testzip() is None
    for name, digest in records.items():
        assert hashlib.sha256(z.read('MyVCS-P1/'+name)).hexdigest()==digest,name
digest=hashlib.sha256(destination.read_bytes()).hexdigest()
(out/'SHA256SUMS.txt').write_text(f'{digest}  {destination.name}\n',encoding='utf-8')
print('PASS ZIP CRC and all member hashes:',len(files),'files,',destination.stat().st_size,'bytes')
print('SHA256:',digest)
