#!/usr/bin/env python3
"""Row 187: real edge cancellation, then commit, on an already-reached screen.

Use an owned API 34+ full-size emulator, gesture navigation, dev debug APK and
an isolated QA Claktory. Set up the actual screen first; this script never
builds, installs, launches, pairs, sends codes, or changes device settings.
Example (labels remain private; evidence contains only counts and hashes):

  python3 app/integration_test/android/predictive_back_gate.py surface \
    --own-emulator-slot --serial emulator-5570 --source "$REV" \
    --apk-sha256 "$SHA" --case sign-in --expect "Sign in to connect to $NAME" \
    --saved-claktory "$NAME" --out app/build/android-back/sign-in.json

Run again on Add a Claktory, using a unique body control such as
--expect 'Use a code instead' (not the generic Claktories/Add labels).
Each run cancels three times, asserting the named screen and Back affordance
remain, then commits once, asserting the actual Claktories list. The Settings
control command uses the same cancellation on an already-open Settings screen
with --expect naming it; it neither opens Settings nor returns to Claktory.

Touches use the emulator console's `event mouse` with device 0 (touchscreen),
one continuous kernel contact, rather than separate `input motionevent`
processes which reset event downTime. No root, writable /dev/input, console
auth-token access, new APK or fake channel is used; adb handles console auth.
A native PredictiveBackTest must separately prove OS start/cancel delivery:
retaining a screen alone cannot prove that an edge gesture was recognized.
Only portrait, non-overridden, full-size displays and one idle touchscreen are
supported. Console touch can prompt Gboard's "Try out your stylus" on the next
text field; dismiss it before typing. This does not certify finger-versus-stylus
behavior. Physical/OEM acceptance and the recording-root control remain
separate. Evidence stays in this checkout's ignored app/build; no UI XML,
labels, account details or screenshots are retained. Run self-test without adb.
"""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time
import uuid
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[3]
PACKAGE = 'com.claktory.app.dev'


class Refused(Exception):
    pass


def need(value, message):
    if not value:
        raise Refused(message)


def run(command, timeout=30):
    result = subprocess.run(command, capture_output=True, timeout=timeout)
    need(result.returncode == 0, 'Tool failed; sensitive output withheld')
    return result.stdout.decode('utf-8', errors='replace')


def adb(args, *command):
    return run([args.adb, '-s', args.serial, *command])


def component(args):
    state = adb(args, 'shell', 'dumpsys activity activities')
    for line in state.splitlines():
        if 'topResumedActivity' in line or 'mResumedActivity' in line:
            found = re.search(r'\b([\w.]+/[\w.$]+)', line)
            if found:
                return found[1]
    raise Refused('No foreground activity')


def labels(args, package):
    path = '/data/local/tmp/claktory-back-' + uuid.uuid4().hex + '.xml'
    data = adb(args, 'shell', f"umask 077; trap 'rm -f {path}' EXIT; "
               f": > {path}; chmod 600 {path}; "
               f"uiautomator dump {path} >/dev/null && cat {path}")
    first, last = data.find('<?xml'), data.rfind('</hierarchy>')
    need(first >= 0 and last >= 0, 'Android semantics unavailable')
    tree = ET.fromstring(data[first:last + len('</hierarchy>')])
    return [value for node in tree.iter('node') if node.get('package') == package
            for key in ('text', 'content-desc', 'hint')
            if (value := node.get(key, ''))]


def contains(values, text):
    return any(text in value for value in values)


def is_list(values):
    return ('Claktories' in values and contains(values, 'Add a Claktory')
            and not contains(values, 'Back to Claktories'))


def surface(values, expected):
    need(all(contains(values, text) for text in expected),
         'Named surface is missing or changed')
    need(contains(values, 'Back to Claktories') and not is_list(values),
         'Expected a full-screen Claktories Back affordance, not the list')


def gesture(width, height, returning):
    # Physical pixels. The emulator maps these to its touchscreen ABS ranges.
    edge, inward = 2, round(width * (350 if returning else 500) / 1080)
    y = height // 2
    points = [(edge, y, 1)]
    points += [(round(edge + (inward - edge) * i / 20), y, 1) for i in range(1, 21)]
    if returning:
        points += [(round(inward - (inward - edge) * i / 20), y, 1) for i in range(1, 21)]
        points += [(edge, y, 1)] * 10  # Hold at the edge before lifting.
    points.append((edge if returning else inward, y, 0))
    return points


def inject(args, width, height, returning):
    points = gesture(width, height, returning)
    x, y, _ = points[0]
    try:
        for index, (x, y, buttons) in enumerate(points):
            if index:
                time.sleep(0.016)  # Gesture pacing, not a wait for task completion.
            answer = adb(args, 'emu', 'event', 'mouse', str(x), str(y), '0', str(buttons))
            need(answer.strip() == 'OK', 'Emulator touchscreen event was refused')
    finally:
        # Preserve one contact across MOVE commands, and always lift on failure.
        adb(args, 'emu', 'event', 'mouse', str(x), str(y), '0', '0')


def preflight(args):
    need(args.own_emulator_slot and re.fullmatch(r'emulator-\d+', args.serial),
         'Explicit ownership of a local emulator is required')
    need(not os.environ.get('ADB_SERVER_SOCKET')
         and not os.environ.get('ANDROID_ADB_SERVER_ADDRESS'), 'Remote adb is not allowed')
    need(adb(args, 'shell', 'getprop ro.kernel.qemu').strip() == '1', 'Not an emulator')
    need(adb(args, 'shell', 'id -u').strip() == '2000', 'Use ordinary adb shell, not adb root')
    need(int(adb(args, 'shell', 'getprop ro.build.version.sdk').strip()) >= 34,
         'Android 14+ required for predictive cancellation')
    need(adb(args, 'shell', 'settings get secure navigation_mode').strip() == '2',
         'Gesture navigation must already be enabled')
    need(re.fullmatch(r'[0-9a-f]{40}', args.source), 'Full source revision required')
    need(re.fullmatch(r'[0-9a-f]{64}', args.apk_sha256), 'Installed APK SHA-256 required')
    need('DEBUGGABLE' in adb(args, 'shell', 'dumpsys package ' + PACKAGE),
         'Only the dev debug app is allowed')
    path = adb(args, 'shell', 'pm path ' + PACKAGE).strip()
    need(bool(re.fullmatch(r'package:/data/app/[\w/+=.~\-]+/base\.apk', path)),
         'Need a single installed dev APK')
    actual = adb(args, 'shell', 'sha256sum ' + path.removeprefix('package:')).split()[0]
    need(actual == args.apk_sha256, 'Installed APK does not match recorded artifact')
    size = adb(args, 'shell', 'wm size')
    found = re.fullmatch(r'Physical size: (\d+)x(\d+)\s*', size)
    need(found is not None, 'Physical display required; size overrides are not supported')
    width, height = map(int, found.groups())
    need(width >= 1080 and height > width, 'Full-size portrait emulator required')
    state = adb(args, 'shell', 'dumpsys input')
    direct = re.findall(r'Touch Input Mapper \(mode - DIRECT\):(.+?)(?=\n  Device |\Z)',
                        state, re.DOTALL)
    need(len(direct) == 1 and re.search(r'Viewport INTERNAL: displayId=0,.*orientation=0,', direct[0]),
         'One portrait touchscreen on the default display is required')
    need('Last Cooked Touch: pointerCount=0' in direct[0], 'Touchscreen is not idle')
    ime = adb(args, 'shell', 'dumpsys input_method')
    need(bool(re.search(r'\bmInputShown=false\b', ime)), 'Close the keyboard before this gate')
    help_text = adb(args, 'emu', 'help', 'event mouse')
    need(bool(re.search(r'\b0\s*=\s*touch screen', help_text)), 'Console touchscreen injection unavailable')
    return width, height


def execute(args):
    need(args.expect and all(text.strip() and text not in ('Claktories', 'Add a Claktory')
                            for text in args.expect), 'Unique named surface assertions required')
    need(re.fullmatch(r'[a-z0-9][a-z0-9-]{0,39}', args.case), 'Nonprivate case slug required')
    out = args.out.resolve()
    need(out.is_relative_to((ROOT / 'app/build').resolve()), 'Evidence must stay in app/build')
    need(not out.exists(), 'Use a new evidence file for each run')
    width, height = preflight(args)
    package = PACKAGE if args.command == 'surface' else 'com.android.settings'
    before = component(args)
    need(before.startswith(package + '/'), 'Expected app is not in the foreground')
    values = labels(args, package)
    if args.command == 'surface':
        surface(values, args.expect)
    else:
        need(all(contains(values, text) for text in args.expect), 'Named Settings surface missing')
    for _ in range(3):
        need(component(args) == before, 'Foreground changed before cancellation')
        inject(args, width, height, returning=True)
        values = labels(args, package)  # uiautomator waits for settled UI.
        need(component(args) == before, 'Cancellation left the activity')
        if args.command == 'surface':
            surface(values, args.expect)
        else:
            need(all(contains(values, text) for text in args.expect), 'Settings cancellation navigated')
    if args.command == 'surface':
        inject(args, width, height, returning=False)
        values = labels(args, PACKAGE)
        need(component(args) == before and is_list(values), 'Commit did not reach the Claktories list')
        need(not any(contains(values, text) for text in args.expect), 'Outgoing surface still visible')
        if args.saved_claktory:
            need(contains(values, args.saved_claktory), 'Saved Claktory missing from list')
    receipt = {'result': 'PASS', 'case': args.case, 'source': args.source,
               'installed_apk_sha256': args.apk_sha256, 'command': args.command,
               'cancellations': 3, 'commits': int(args.command == 'surface'),
               'named_assertions': len(args.expect), 'saved_claktory_asserted': bool(args.saved_claktory),
               'injection': 'emulator-console-continuous-touchscreen', 'physical_device': False}
    out.parent.mkdir(parents=True, exist_ok=True)
    with os.fdopen(os.open(out, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), 'w') as output:
        json.dump(receipt, output, indent=2)
    print('PASS: three cancellations retained the named surface' +
          ('; commit reached the Claktories list' if args.command == 'surface' else ''))


def self_test():
    cancel = gesture(1080, 2400, True)
    commit = gesture(1080, 2400, False)
    assert len(cancel) == 52 and len(commit) == 22
    assert cancel[0] == (2, 1200, 1) and cancel[-1] == (2, 1200, 0)
    assert max(point[0] for point in cancel) == 350
    assert commit[-1] == (500, 1200, 0)
    assert all(point[2] == 1 for point in cancel[:-1])
    assert cancel[-11:-1] == [(2, 1200, 1)] * 10
    assert gesture(2160, 4800, False)[-1] == (1000, 2400, 0)
    assert is_list(['Claktories', 'Add a Claktory', 'qa-test'])
    assert not is_list(['Claktories', 'Add a Claktory', 'Back to Claktories'])
    surface(['Sign in to connect to qa-test', 'Back to Claktories'], ['Sign in to connect to qa-test'])
    try:
        surface(['Claktories', 'Add a Claktory'], ['Sign in to connect to qa-test'])
    except Refused:
        pass
    else:
        raise AssertionError('List accepted as retained blocked screen')
    from types import SimpleNamespace
    from unittest.mock import patch

    sent = []

    def fake_touch(_args, *command):
        sent.append(command)
        return 'OK\r\n'

    with patch.dict(globals(), {'adb': fake_touch}), patch.object(time, 'sleep'):
        inject(None, 1080, 2400, True)
    assert len(sent) == 53
    assert sent[0][-1] == '1' and all(command[-1] == '1' for command in sent[:-2])
    assert sent[-1][-1] == sent[-2][-1] == '0'
    sent.clear()

    def failing_touch(_args, *command):
        sent.append(command)
        if len(sent) == 3:
            raise Refused('injection failed')
        return 'OK'

    with patch.dict(globals(), {'adb': failing_touch}), patch.object(time, 'sleep'):
        try:
            inject(None, 1080, 2400, True)
        except Refused:
            pass
        else:
            raise AssertionError('Failed injection accepted')
    assert len(sent) == 4 and sent[-1][-1] == '0'

    args = SimpleNamespace(own_emulator_slot=True, serial='emulator-5570',
                           source='a' * 40, apk_sha256='b' * 64)
    responses = {
        ('shell', 'getprop ro.kernel.qemu'): '1',
        ('shell', 'id -u'): '2000',
        ('shell', 'getprop ro.build.version.sdk'): '36',
        ('shell', 'settings get secure navigation_mode'): '2',
        ('shell', 'dumpsys package ' + PACKAGE): 'pkgFlags=[ DEBUGGABLE HAS_CODE ]',
        ('shell', 'pm path ' + PACKAGE): 'package:/data/app/~~abc/abc/base.apk',
        ('shell', 'sha256sum /data/app/~~abc/abc/base.apk'): 'b' * 64 + '  base.apk',
        ('shell', 'wm size'): 'Physical size: 1080x2400\n',
        ('shell', 'dumpsys input'): ('Touch Input Mapper (mode - DIRECT):\n'
                                     'Viewport INTERNAL: displayId=0, orientation=0,\n'
                                     'Last Cooked Touch: pointerCount=0'),
        ('shell', 'dumpsys input_method'): 'mInputShown=false',
        ('emu', 'help', 'event mouse'): '0  = touch screen',
    }
    with patch.dict(globals(), {'adb': lambda _args, *command: responses[command]}), \
            patch.dict(os.environ, {}, clear=True):
        assert preflight(args) == (1080, 2400)
        for key, bad in [
            (('shell', 'getprop ro.kernel.qemu'), '0'),
            (('shell', 'id -u'), '0'),
            (('shell', 'getprop ro.build.version.sdk'), '33'),
            (('shell', 'settings get secure navigation_mode'), '0'),
            (('shell', 'dumpsys package ' + PACKAGE), 'HAS_CODE'),
            (('shell', 'wm size'), 'Physical size: 320x640\nOverride size: 1080x2400'),
            (('shell', 'dumpsys input_method'), 'mInputShown=true'),
            (('shell', 'sha256sum /data/app/~~abc/abc/base.apk'), 'c' * 64),
        ]:
            with patch.dict(responses, {key: bad}):
                try:
                    preflight(args)
                except Refused:
                    pass
                else:
                    raise AssertionError('Unsafe/incorrect fixture accepted')
    print('PASS: contact lifetime/cleanup, preflight refusals and named-surface tripwires (no device claim)')


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest='command', required=True)
    sub.add_parser('self-test')
    for command in ('surface', 'settings-control'):
        child = sub.add_parser(command)
        child.add_argument('--own-emulator-slot', action='store_true')
        child.add_argument('--serial', required=True)
        child.add_argument('--adb', default='adb')
        child.add_argument('--source', required=True)
        child.add_argument('--apk-sha256', required=True)
        child.add_argument('--case', required=True)
        child.add_argument('--expect', action='append', default=[])
        child.add_argument('--saved-claktory')
        child.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    try:
        if args.command == 'self-test':
            self_test()
        else:
            execute(args)
        return 0
    except Refused as error:
        print('FAIL: ' + str(error), file=sys.stderr)
    except Exception:
        print('FAIL: operation failed; sensitive output withheld', file=sys.stderr)
    return 1


if __name__ == '__main__':
    sys.exit(main())
