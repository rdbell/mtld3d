#!/usr/bin/env python3
"""Bounded multi-device and early window-destruction checks in an isolated prefix."""
import argparse,json,os,shutil,subprocess,time
from pathlib import Path
p=argparse.ArgumentParser(description=__doc__)
for key in ('wine','prefix','probe','dll','control','output'):p.add_argument('--'+key,type=Path,required=True)
p.add_argument('--expect-host',choices=['yes','no'],default='yes')
p.add_argument('--native-host',choices=['true','false'],default='true')
a=p.parse_args()
for k,v in vars(a).items():
 if isinstance(v,Path):setattr(a,k,v.resolve())
if not a.prefix.is_relative_to(a.output.parent):p.error('prefix must be task-local')
a.output.mkdir(exist_ok=False)
env=dict(os.environ,WINEPREFIX=str(a.prefix),WINEDLLOVERRIDES='d3d9=n,b',WINEDEBUG='-all',MTLD3D_CONFIG='color.hdr.enable=false;present.nativeHost='+a.native_host)
for key in list(env):
 if key.startswith(('X87_','DYLD_')):env.pop(key)
results=[]
for mode in ('first-first','second-first','window-first'):
 out=a.output/mode;out.mkdir();shutil.copy2(a.probe,out/'native-probe.exe');shutil.copy2(a.dll,out/'d3d9.dll')
 observed=set();deadline=time.monotonic()+45
 with (out/'probe.log').open('w') as log:
  child=subprocess.Popen([str(a.wine),str(out/'native-probe.exe'),mode],cwd=out,env=env,stdout=log,stderr=subprocess.STDOUT)
  try:
   while child.poll() is None:
    if time.monotonic()>deadline:raise TimeoutError('45-second lifecycle limit')
    text=(out/'probe.log').read_text(errors='replace')
    stages=[line for line in text.splitlines() if line.startswith('lifecycle ')]
    if stages and stages[-1] not in observed:
     stage=stages[-1]
     time.sleep(.6)
     rows=json.loads(subprocess.check_output([str(a.control),str(child.pid),'list'],text=True,timeout=5))
     hosted=any(x.get('kCGWindowName')=='Final Fantasy XI' and x.get('kCGWindowIsOnscreen') for x in rows)
     expected=a.expect_host=='yes' and stage in ('lifecycle both alive','lifecycle second released','lifecycle recreated')
     assert hosted==expected,(mode,stage,rows)
     observed.add(stage);results.append(dict(mode=mode,stage=stage,hosted=hosted))
    time.sleep(.05)
   text=(out/'probe.log').read_text(errors='replace')
   assert child.returncode==0 and 'teardown complete' in text,(child.returncode,text)
   native_log=''.join(x.read_text(errors='replace') for x in (out/'mtld3d-logs').glob('*.log'))
   expected_attaches=1 if mode=='window-first' else 2
   if a.expect_host=='yes':
    assert native_log.count('native host: attached window=')==expected_attaches,native_log
    assert native_log.count('native host: detached and restored')==expected_attaches,native_log
   else:
    assert 'native host: attached window=' not in native_log,native_log
   required={'lifecycle both alive' ,'lifecycle owner released'}
   required.add('lifecycle window destroyed' if mode=='window-first' else 'lifecycle recreated')
   if mode=='second-first':required.add('lifecycle second released')
   assert required<=observed,(required,observed)
  finally:
   subprocess.run([str(a.wine.parent/'wineserver'),'-k'],env=env,timeout=10,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
   child.wait(timeout=10)
(a.output/'results.json').write_text(json.dumps(results,indent=2)+'\n')
print(f'{len(results)} lifecycle checkpoints passed')
