"""Paired GC experiment; same input repositories, preserve all runs and history hashes."""
import argparse,hashlib,json,os,platform,random,shutil,subprocess,tempfile,time,zlib
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--old',required=True);p.add_argument('--new',required=True);p.add_argument('--out',required=True);p.add_argument('--rounds',type=int,default=3);a=p.parse_args()
old,new=map(lambda s: str(Path(s).resolve()),(a.old,a.new));out=Path(a.out).resolve();out.mkdir(parents=True,exist_ok=True)
env={k:v for k,v in os.environ.items() if not k.startswith('MYVCS_')};env['MYVCS_FORCE_DEEP_DELTA']='1'
def run(bin,repo,*args):
 r=subprocess.run([bin,'--repo',str(repo),*args],env=env,stdout=subprocess.PIPE,stderr=subprocess.PIPE,check=True);return r.stdout.decode()
def measure(bin,repo,*args):
 f=out/'time.tmp';t=time.perf_counter_ns();r=subprocess.run(['/usr/bin/time','-f','%M','-o',str(f),bin,'--repo',str(repo),*args],env=env,capture_output=True,check=True);elapsed=(time.perf_counter_ns()-t)/1e6
 return {'elapsed_ms':elapsed,'max_rss_kib':int(f.read_text().strip()),'stdout':r.stdout.decode(),'stderr':r.stderr.decode()}
manifest={'platform':platform.platform(),'os_release':Path('/etc/os-release').read_text(),'cpu':Path('/proc/cpuinfo').read_text(),'memory':Path('/proc/meminfo').read_text(),'seed':28102026,'versions':18,'rounds':a.rounds,'warmups':0,'cache_policy':'not dropped; fixture creation and copying warm cache','timing':'wall time includes GNU time and command startup; RSS per process peak','old_binary_sha256':hashlib.sha256(Path(old).read_bytes()).hexdigest(),'new_binary_sha256':hashlib.sha256(Path(new).read_bytes()).hexdigest()}
(out/'manifest.json').write_text(json.dumps(manifest,indent=2))
records=[]
with tempfile.TemporaryDirectory(prefix='myvcs-p1-bench-') as tmp:
 root=Path(tmp)
 for scenario in ['text','binary','unrelated-random']:
  fixture=root/scenario;subprocess.run([old,'init','--empty',str(fixture)],env=env,check=True,capture_output=True)
  rng=random.Random(28102026);data=bytearray((b'alpha beta gamma delta\n'*12000) if scenario=='text' else rng.randbytes(1024*1024));versions=[]
  for i in range(18):
   if scenario=='unrelated-random':data=bytearray(rng.randbytes(256*1024))
   else:data[100+i*100:108+i*100]=(f'{i:08d}'.encode() if scenario=='text' else rng.randbytes(8))
   (fixture/'data').write_bytes(data);run(old,fixture,'add','data');run(old,fixture,'commit','-m',f'version {i}')
   oid=(fixture/'.myvcs/refs/heads/main').read_text().strip();versions.append({'commit':oid,'sha256':hashlib.sha256(data).hexdigest()})
  (out/(scenario+'-versions.json')).write_text(json.dumps(versions,indent=2))
  for trial in range(a.rounds):
   for mode,bin in ([('old',old),('new',new)] if trial%2==0 else [('new',new),('old',old)]):
    repo=root/f'{scenario}-{trial}-{mode}';shutil.copytree(fixture,repo)
    gc=measure(bin,repo,'gc');verify=measure(bin,repo,'verify');stats=json.loads(run(new,repo,'stats','--json'))
    restored=[];elapsed=0.0;peak=0
    for v in versions:
     m=measure(bin,repo,'checkout',v['commit']);elapsed+=m['elapsed_ms'];peak=max(peak,m['max_rss_kib']);actual=hashlib.sha256((repo/'data').read_bytes()).hexdigest();assert actual==v['sha256'];restored.append(actual)
    row={'scenario':scenario,'trial':trial+1,'mode':mode,'gc':gc,'verify':verify,'stats':stats,'all_history_checkout_ms':elapsed,'checkout_peak_rss_kib':peak,'restored_hashes':restored}
    records.append(row);(out/'results.json').write_text(json.dumps(records,indent=2));print(scenario,trial+1,mode,'depth',stats['max_reachable_chain_nodes'],'bytes',stats['object_storage_bytes'],'GC_ms',round(gc['elapsed_ms'],2),'all history verified',flush=True);shutil.rmtree(repo)
print('PASS paired GC benchmark and all historical bytes')
