#!/usr/bin/env python3
"""Exercise native window presentation with a bounded, isolated Wine probe."""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import time

parser = argparse.ArgumentParser(description=__doc__)
for option in ('wine', 'prefix', 'probe', 'dll', 'control', 'output'):
    parser.add_argument('--' + option, type=Path, required=True)
args = parser.parse_args()
for name, value in vars(args).items():
    setattr(args, name, value.resolve())
if not args.prefix.is_relative_to(args.output.parent):
    parser.error('the test prefix must be inside the output parent directory')
args.output.mkdir(parents=True, exist_ok=False)
shutil.copy2(args.probe, args.output / 'native-probe.exe')
shutil.copy2(args.dll, args.output / 'd3d9.dll')
env = os.environ.copy()
for key in list(env):
    if key.startswith(('X87_', 'DYLD_')):
        env.pop(key)
env.update(WINEPREFIX=str(args.prefix), WINEDLLOVERRIDES='d3d9=n,b',
           MTLD3D_CONFIG='color.hdr.enable=false;present.maxFps=60;present.nativeHost=true',
           WINEDEBUG='-all', RUST_LOG='mtld3d=info,mtld3d::native_host=debug')
log_path = args.output / 'probe.log'
deadline = time.monotonic() + 120
results = []


def remaining(cap=8):
    seconds = min(cap, deadline - time.monotonic())
    if seconds <= 0:
        raise TimeoutError('120-second probe deadline exceeded')
    return seconds


def control(*command):
    return subprocess.check_output([str(args.control), str(process.pid), *map(str, command)],
                                   text=True, timeout=remaining())


def log():
    return log_path.read_text(errors='replace')


def wait_for(predicate, cap=8):
    end = time.monotonic() + remaining(cap)
    while time.monotonic() < end:
        if predicate():
            return
        if process.poll() is not None:
            raise RuntimeError('probe exited before acceptance')
        time.sleep(.1)
    raise TimeoutError('probe acceptance condition timed out')


def windows():
    return json.loads(control('list'))


def capture(label):
    rows = windows()
    host = next(row for row in rows if row.get('kCGWindowName') == 'Final Fantasy XI')
    path = args.output / (label + '.png')
    subprocess.run(['screencapture', '-x', '-o', '-l', str(host['kCGWindowNumber']), str(path)],
                   check=True, timeout=remaining())
    pixel_check = control('pixels', path).strip()
    results.append({'stage': label, 'windows': rows, 'pixels': pixel_check})


def keyboard():
    wait_for(lambda: re.findall(r'foreground (\d)', log())[-1:] == ['1'])
    print('keyboard state', control('state'), flush=True)
    before=log().count('key 65')
    control('key', 0, 'none')
    wait_for(lambda: log().count('key 65') > before)
    wait_for(lambda: re.findall(r'foreground (\d)', log())[-1:] == ['1'])


def title_focus():
    host=next(row for row in windows() if row.get('kCGWindowName') == 'Final Fantasy XI')
    rect=host['kCGWindowBounds']
    control('click',rect['X']+rect['Width']/2,rect['Y']+12)
    time.sleep(.8)
    if 'active false' in control('state'): control('activate')
    keyboard()


def click_quarter():
    child = next(row for row in windows() if row.get('kCGWindowName') == 'Native host probe')
    rect = child['kCGWindowBounds']
    count = log().count('click ')
    control('click', rect['X'] + rect['Width'] / 4, rect['Y'] + rect['Height'] / 4)
    wait_for(lambda: log().count('click ') > count)
    match = re.findall(r'click (\d+) (\d+) cursor (\d+) (\d+) client (\d+) (\d+)', log())[-1]
    x, y, cx, cy, width, height = map(int, match)
    assert abs(x - width / 4) < 5 and abs(y - height / 4) < 5, match
    assert abs(x - cx) < 5 and abs(y - cy) < 5, match


with log_path.open('w') as output:
    process = subprocess.Popen([str(args.wine), str(args.output / 'native-probe.exe')],
                               env=env, cwd=args.output, stdout=output, stderr=subprocess.STDOUT)
    try:
        wait_for(lambda: 'frame ' in log(), 40)
        title_focus()
        for index, rect in enumerate([(100, 100, 1280, 748), (100, 100, 1800, 1128),
                                      (-1300, 1050, 1000, 678), (-1300, 1050, 1280, 808),
                                      (100, 100, 1280, 808)]):
            control('resize', *rect)
            time.sleep(.7)
            capture('resize-' + str(index))
            click_quarter()
        control('key', 0, 'none')
        wait_for(lambda: 'key 65' in log())
        control('key', 43, 'command')
        wait_for(lambda: any(row.get('kCGWindowName') == 'Graphics Settings' for row in windows()))
        control('settings-value', 30)
        time.sleep(3.5)
        samples = re.findall(r'frame (\d+)', log())[-2:]
        assert len(samples) == 2 and all(26 <= int(value) <= 34 for value in samples), samples
        results.append({'stage': 'live-frame-limit', 'samples': samples})
        control('settings-value', 60)
        time.sleep(3.5)
        samples = re.findall(r'frame (\d+)', log())[-2:]
        assert len(samples)==2 and all(53 <= int(value) <= 67 for value in samples), samples
        results.append({'stage':'restored-frame-limit','samples':samples})
        control('settings-close')
        time.sleep(.3)
        keyboard()
        control('minimize')
        wait_for(lambda: 'minimize 1' in log())
        capture('minimize-cancelled')
        control('minimize')
        time.sleep(.8)
        assert not any(row.get('kCGWindowIsOnscreen', False) for row in windows()), windows()
        wait_for(lambda: 'size 1 iconic 1' in log())
        control('restore')
        time.sleep(.8)
        capture('restored')
        keyboard()
        before=log().count('size 1 iconic 1')
        control('key',46,'none') # M: Win32 minimize, timed restore
        wait_for(lambda: log().count('size 1 iconic 1') > before)
        time.sleep(2)
        capture('win32-restored')
        title_focus()
        control('fullscreen')
        time.sleep(1.5)
        capture('fullscreen')
        keyboard()
        invalid_count=log().count('invalid 1 result 8876086c')
        control('key',34,'none')
        wait_for(lambda: log().count('invalid 1 result 8876086c') > invalid_count)
        assert control('is-fullscreen').strip() in ('1','true'), 'rejected Reset lost native fullscreen'
        capture('fullscreen-rejected-reset')
        invalid_count=log().count('invalid 2 result 8876086c')
        control('key',2,'none') # D: invalid auto-depth format
        wait_for(lambda: log().count('invalid 2 result 8876086c') > invalid_count)
        assert control('is-fullscreen').strip() in ('1','true'), 'invalid depth Reset lost native fullscreen'
        capture('fullscreen-rejected-depth-reset')
        control('windowed')
        time.sleep(1.5)
        capture('windowed')
        keyboard()
        control('key',43,'command')
        wait_for(lambda: any(row.get('kCGWindowName')=='Graphics Settings' for row in windows()))
        control('settings-value',30)
        control('settings-close')
        time.sleep(.8)
        keyboard()
        invalid_count=log().count('invalid 1 result 8876086c')
        control('key',34,'none') # I: invalid exclusive Reset preserves host
        wait_for(lambda: log().count('invalid 1 result 8876086c') > invalid_count)
        capture('invalid-reset-preserved')
        time.sleep(2.5)
        assert all(26 <= int(v) <= 34 for v in re.findall(r'frame (\d+)', log())[-2:])
        keyboard()
        control('key',14,'none') # E: exclusive D3D Reset removes host
        wait_for(lambda: 'reset windowed 0 invalid 0 result 00000000' in log())
        time.sleep(.6)
        assert not any(row.get('kCGWindowName')=='Final Fantasy XI' and row.get('kCGWindowIsOnscreen') for row in windows()), windows()
        control('key',13,'none') # W: return to hosted windowed presentation
        wait_for(lambda: 'reset windowed 1 invalid 0 result 00000000' in log())
        time.sleep(.6)
        capture('reset-windowed')
        time.sleep(2.5)
        assert all(26 <= int(v) <= 34 for v in re.findall(r'frame (\d+)', log())[-2:])
        title_focus()
        click_quarter()
        control('close')
        wait_for(lambda: 'close 1' in log())
        capture('close-cancelled')
        control('close')
        process.wait(timeout=remaining(10))
        assert process.returncode == 0, process.returncode
        assert 'teardown complete' in log() and 'FAIL' not in log(), log()
        results.append({'stage': 'teardown', 'exit_code': process.returncode})
    finally:
        subprocess.run([str(args.wine.parent / 'wineserver'), '-k'], env=env, timeout=10,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        (args.output / 'results.json').write_text(json.dumps(results, indent=2) + '\n')
print(json.dumps({'passed': True, 'stages': len(results)}))
