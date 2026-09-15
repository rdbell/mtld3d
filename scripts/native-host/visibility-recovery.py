"""Inject one stale hidden bit into our own standalone probe; compare automatic recovery."""
import os,time,json,re,subprocess,shutil
from pathlib import Path
import argparse
parser=argparse.ArgumentParser(description=__doc__)
for option in ('wine','prefix','probe','bundle','injector','output'):
 parser.add_argument('--'+option,type=Path,required=True)
parser.add_argument('--expect',choices=('stuck','recover'),required=True)
args=parser.parse_args()
for name,value in vars(args).items():
 if isinstance(value,Path):setattr(args,name,value.resolve())
out=args.output;package=args.bundle;expect_recovery=args.expect=='recover'
sdk=args.wine.parent.parent;prefix=args.prefix
if not sdk.is_relative_to(out.parent) or not prefix.is_relative_to(out.parent):
 parser.error('Wine SDK and prefix must be disposable copies inside the output parent')
out.mkdir(exist_ok=False)
v=package/'Contents/Resources';module=sdk/'lib/wine/x86_64-unix/mtld3d.so'
for rel in ['i386-windows/mtld3d.dll','x86_64-unix/mtld3d.so']:shutil.copy2(v/'mtld3d/wine'/rel,sdk/'lib/wine'/rel)
shutil.copy2(v/'wine-native-host/winemac.so',sdk/'lib/wine/x86_64-unix/winemac.so')
shutil.copy2(args.probe,out/'native-probe.exe');shutil.copy2(v/'mtld3d/native/i386-windows/d3d9.dll',out/'d3d9.dll')
env={k:v for k,v in os.environ.items() if not k.startswith(('X87_','DYLD_','MTLD3D_','WINEDLLPATH'))}
env.update(WINEPREFIX=str(prefix),WINEDLLOVERRIDES='d3d9=n,b',MTLD3D_CONFIG='color.hdr.enable=false;present.maxFps=60;present.nativeHost=true',WINEDEBUG='-all',RUST_LOG='mtld3d=info,mtld3d::unix::present=trace')
deadline=time.monotonic()+90
def run(argv,timeout=8):
 remaining=min(timeout,deadline-time.monotonic())
 if remaining<=0:raise TimeoutError('90-second probe bound exceeded')
 return subprocess.check_output(list(map(str,argv)),text=True,timeout=remaining)
p=None;result={'passed':False}
try:
 with (out/'probe.log').open('w') as f:p=subprocess.Popen([str(args.wine),str(out/'native-probe.exe')],cwd=out,env=env,stdout=f,stderr=subprocess.STDOUT)
 end=time.monotonic()+35
 while time.monotonic()<end and 'frame ' not in (out/'probe.log').read_text(errors='replace'):time.sleep(.2)
 assert 'frame ' in (out/'probe.log').read_text(errors='replace'),'no frames'
 time.sleep(1)
 assert 'foreground 1' in (out/'probe.log').read_text(errors='replace').splitlines()[-1], 'probe must be frontmost'
 # PID is our direct child; refuse if either executable or renderer mapping differs.
 mapped=run(['lsof','-p',p.pid,'-Fn']);assert '\nn'+str(out/'native-probe.exe')+'\n' in mapped and '\nn'+str(module)+'\n' in mapped
 run(['sample',p.pid,1,20,'-file',out/'identity-sample.txt'],15)
 sample=(out/'identity-sample.txt').read_text();matches=re.findall(r'^\s*(0x[0-9a-f]+) -\s*0x[0-9a-f]+ \+mtld3d.so',sample,re.M);assert len(matches)==1,matches
 nm=run(['nm','-an',module]);symbols=re.findall(r'^([0-9a-f]+) b .*WINDOW_OCCLUDED.*$',nm,re.M);assert len(symbols)==1,symbols
 address=int(matches[0],16)+int(symbols[0],16)
 log=next((out/'mtld3d-logs').glob('*.log'))
 injection=run([args.injector,p.pid,f'{address:x}',1]);assert 'original=0' in injection,injection
 before=log.read_text().count('presented:')
 time.sleep(4)
 during=log.read_text().count('presented:')-before
 restored=run([args.injector,p.pid,f'{address:x}',0]);time.sleep(.5)
 result={'pid':p.pid,'injection':injection.strip(),'read_and_restore':restored.strip(),'presented_frames_in_4s':during,'expected_recovery':expect_recovery}
 assert ('original=0' in restored)==expect_recovery,result
 assert (during>30) if expect_recovery else (during<5),result
 result['passed']=True
finally:
 if p:
  p.terminate()
  try:p.wait(timeout=5)
  except subprocess.TimeoutExpired:p.kill();p.wait(timeout=5)
 subprocess.run([str(sdk/'bin/wineserver'),'-k'],env=env,timeout=10,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
 (out/'result.json').write_text(json.dumps(result,indent=2)+'\n')
print(json.dumps(result))
