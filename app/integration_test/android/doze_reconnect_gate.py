#!/usr/bin/env python3
"""Emulator-only #214 measurements; see doze_reconnect_gate.md.

Never installs, pairs, approves, fake-resumes, or declares overall acceptance.
Only begin/resume mutate Android, in an explicitly handed-over emulator slot.
Raw UI/logcat remains in memory. Records live in this checkout's ignored build.
"""
import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import re
import subprocess
import time
import uuid
import xml.etree.ElementTree as ET


class Refused(Exception):
    pass


def need(value, message):
    if not value:
        raise Refused(message)


def run(command, timeout=30):
    p = subprocess.run(command, capture_output=True, timeout=timeout)
    need(p.returncode == 0, "Tool failed; sensitive output withheld")
    return p.stdout.decode('utf-8', errors='replace')


def adb(args, *command, timeout=30):
    return run([args.adb, '-s', args.serial, *command], timeout=timeout)


def owned(path):
    path = path.resolve()
    root = (Path.cwd() / 'app/build').resolve()
    need(path.is_relative_to(root), "Evidence/log must be in this checkout's app/build")
    return path


def offset(path):
    stat = path.stat()
    return {'size': stat.st_size, 'inode': stat.st_ino}


def tail(path, mark):
    stat = path.stat()
    need(stat.st_ino == mark['inode'] and stat.st_size >= mark['size'],
         "Log rotated/truncated; retry this case")
    need(stat.st_size - mark['size'] <= 4 * 1024 * 1024, "Log window too large")
    with path.open('rb') as source:
        source.seek(mark['size'])
        return source.read().decode('utf-8', errors='replace')


def idle(state):
    return bool(re.search(r'\bmState=IDLE\b', state))


def endpoint_lines(text, endpoint, event):
    clean = re.sub(r'\x1b\[[0-9;]*m', '', text)
    return [line for line in clean.splitlines()
            if event in line and re.search(r'\bendpoint=' + re.escape(endpoint) + r'(?:\s|$)', line)]


def timestamp(line):
    match = re.match(r'\s*(\d{4}-\d\d-\d\dT\S+)', line)
    need(match, "Backend log needs UTC timestamps")
    return datetime.fromisoformat(match[1].replace('Z', '+00:00')).timestamp()


def relay_counts(text, endpoint):
    # fmt_short(): first five bytes, ten lower-hex digits. Whole word matters:
    # substring matching 'registering' incorrectly counts unregistering too.
    alias = endpoint[:10]
    clean = re.sub(r'\x1b\[[0-9;]*m', '', text)
    lines = [line for line in clean.splitlines()
             if re.search(r'\bremote_endpoint=' + re.escape(alias) + r'(?:\s|$)', line)]
    return {
        'registrations': sum(bool(re.search(r'\bregistering client\b', line)) for line in lines),
        'duplicates': sum('multiple connections found, deactivating old connection' in line
                          for line in lines),
    }


def visible(args, label, timeout=10):
    temporary = '/data/local/tmp/claktory-doze-' + uuid.uuid4().hex + '.xml'
    text = adb(args, 'shell', f"umask 077; trap 'rm -f {temporary}' EXIT; "
               f": > {temporary}; chmod 600 {temporary}; "
               f"uiautomator dump {temporary} >/dev/null && cat {temporary}", timeout=timeout)
    first, last = text.find('<?xml'), text.rfind('</hierarchy>')
    if first < 0 or last < 0:
        return False
    tree = ET.fromstring(text[first:last + len('</hierarchy>')])
    nodes = list(tree.iter('node'))
    labels = [(node.get('text', ''), node.get('content-desc', ''), node.get('hint', ''))
              for node in nodes if node.get('package') == args.package]
    reached = any(label in values for values in labels)
    reconnecting = any('Reconnecting…' in value for values in labels for value in values)
    resumed = adb(args, 'shell', 'dumpsys activity activities')
    foreground = any(args.package + '/' in line for line in resumed.splitlines()
                     if 'topResumedActivity' in line or 'mResumedActivity' in line)
    return reached and not reconnecting and foreground


def write(path, record):
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix('.new')
    temporary.write_text(json.dumps(record, indent=2, sort_keys=True) + '\n')
    temporary.replace(path)


def check_emulator(args):
    need(args.own_emulator_slot, "Explicit emulator slot ownership is required")
    need(adb(args, 'shell', 'getprop ro.kernel.qemu').strip() == '1',
         "This driver mutates only the agreed emulator, never a physical phone")
    need(args.package in ('com.claktory.app', 'com.claktory.app.dev'), "Wrong package")


def begin(args):
    check_emulator(args)
    state_path = owned(args.record)
    need(not state_path.exists(), "Record already exists; do not overwrite an interval")
    backend_log = owned(args.backend_log)
    relay_log = owned(args.relay_log) if args.relay_log else None
    need(bool(re.fullmatch('[0-9a-f]{64}', args.endpoint)), "Need public app endpoint in hex")
    need(args.transport != 'relay' or relay_log is not None, "Relay case needs relay log")
    before = adb(args, 'shell', 'dumpsys deviceidle')
    need('mForceIdle=true' not in before and not idle(before), "Emulator already idle/forced by another test")
    need('mWakefulness=Awake' in adb(args, 'shell', 'dumpsys power'), "Begin with screen awake")
    need('UPDATES STOPPED' not in adb(args, 'shell', 'dumpsys battery'),
         "Battery simulation already owned by another test")
    need(visible(args, args.connected_label), "Reach the actual connected status before starting")
    record = {
        'status': 'PREPARING', 'package': args.package, 'serial': args.serial,
        'transport': args.transport, 'minimum_seconds': args.seconds,
        'host_load_begin': os.getloadavg(), 'host_logical_cpus': os.cpu_count(),
        'endpoint': args.endpoint, 'connected_label': args.connected_label,
        'backend_log': str(backend_log.relative_to(Path.cwd())),
        'backend_start': offset(backend_log),
        'relay_log': str(relay_log.relative_to(Path.cwd())) if relay_log else None,
        'relay_start': offset(relay_log) if relay_log else None,
        'boot_id': adb(args, 'shell', 'cat /proc/sys/kernel/random/boot_id').strip(),
        'app_pid_before': adb(args, 'shell', 'pidof ' + args.package).strip(),
    }
    try:
        adb(args, 'shell', 'input keyevent KEYCODE_HOME')
        adb(args, 'shell', 'dumpsys battery unplug')
        adb(args, 'shell', 'input keyevent KEYCODE_SLEEP')
        adb(args, 'shell', 'dumpsys deviceidle force-idle')
        need(idle(adb(args, 'shell', 'dumpsys deviceidle')), "Deep Doze was not reached")
        record.update(status='SLEEPING', idle_wall=time.time(), idle_monotonic=time.monotonic())
        write(state_path, record)
    except Exception:
        adb(args, 'shell', 'dumpsys deviceidle unforce')
        adb(args, 'shell', 'dumpsys battery reset')
        raise
    print('RECORDED: real deep idle; schedule resume after the full interval')


def resume(args):
    check_emulator(args)
    path = owned(args.record)
    record = json.loads(path.read_text())
    need(record['status'] == 'SLEEPING', "Not a sleeping record")
    need((record['serial'], record['package']) == (args.serial, args.package), "Wrong emulator/package")
    need(adb(args, 'shell', 'cat /proc/sys/kernel/random/boot_id').strip() == record['boot_id'],
         "Emulator restarted; retry this case")
    elapsed = time.time() - record['idle_wall']
    monotonic = time.monotonic() - record['idle_monotonic']
    need(elapsed >= record['minimum_seconds'], "Full away interval has not elapsed")
    need(abs(elapsed - monotonic) < 2, "Host slept or clock changed; retry with explicit qualification")
    need(idle(adb(args, 'shell', 'dumpsys deviceidle')), "Not still in deep Doze")
    backend_log = owned(Path(record['backend_log']))
    # Refuse log rotation before waking, rather than hiding the missing proof.
    tail(backend_log, record['backend_start'])
    wake_offset = offset(backend_log)
    wake_wall, wake_clock = time.time(), time.monotonic()
    record.update(elapsed_seconds=round(elapsed, 3), idle_verified_before_wake=True,
                  host_load_resume=os.getloadavg(),
                  status='WAKING', wake_backend_offset=wake_offset,
                  wake_utc=datetime.fromtimestamp(wake_wall, timezone.utc).isoformat())
    write(path, record)
    try:
        adb(args, 'shell', 'dumpsys deviceidle unforce')
        adb(args, 'shell', 'dumpsys battery reset')
        adb(args, 'shell', 'input keyevent KEYCODE_WAKEUP')
        # Actual OS dismissal of this test emulator's uncredentialed swipe
        # lock. This is not a fake lifecycle callback or a PIN-lock test.
        adb(args, 'shell', 'wm dismiss-keyguard')
        # Non-exported notification routes are irrelevant here: resume the
        # actual launcher activity, not a fake lifecycle channel callback.
        adb(args, 'shell', 'am start -n ' + args.package + '/com.claktory.app.MainActivity')
        backend_connected = None
        ui_upper_bound = None
        ui_errors = 0
        while time.monotonic() - wake_clock < 15:
            logs = tail(backend_log, wake_offset)
            connections = endpoint_lines(logs, record['endpoint'], 'device connected')
            if connections and backend_connected is None:
                backend_connected = timestamp(connections[0]) - wake_wall
            try:
                reached = visible(args, record['connected_label'], timeout=3)
            except (subprocess.TimeoutExpired, ET.ParseError, Refused):
                # A busy/inaccessible UI is not a connected-UI pass. Still
                # retain independent backend timings/counts instead of losing
                # all measurements to uiautomator's long default idle wait.
                ui_errors += 1
                reached = False
            if reached:
                ui_upper_bound = time.monotonic() - wake_clock
                break
            time.sleep(0.1)
        logs = tail(backend_log, wake_offset)
        opened = endpoint_lines(logs, record['endpoint'], 'tunnel connection opened')
        connected = endpoint_lines(logs, record['endpoint'], 'device connected')
        if backend_connected is None and connected:
            backend_connected = timestamp(connected[0]) - wake_wall
        record.update(
            status='MEASURED_NOT_CERTIFIED',
            backend_connected_seconds=round(backend_connected, 3) if backend_connected is not None else None,
            ui_connected_upper_bound_seconds=round(ui_upper_bound, 3) if ui_upper_bound is not None else None,
            new_tunnel_open_count=len(opened),
            old_drop_count=len(endpoint_lines(tail(backend_log, record['backend_start']), record['endpoint'],
                                               "a device's tunnel connection dropped")),
            tunnel_open_lines=opened,
            device_connected_lines=[re.sub(r'\bdevice=\S+', 'device=[redacted]', line) for line in connected],
            app_pid_after=adb(args, 'shell', 'pidof ' + args.package).strip(),
            backend_five_second_target_met=(backend_connected is not None and 0 <= backend_connected <= 5),
            ui_observation_errors=ui_errors,
            still_owed=['post-resume path', 'nonce assistant roundtrip and replied_to',
                        'approved device/no pending', 'app close/bind logs',
                        'zero relay duplicate/replaced clients; no extra post-resume bind'],
        )
        if record['relay_log']:
            record['relay'] = relay_counts(tail(owned(Path(record['relay_log'])), record['relay_start']),
                                           record['endpoint'])
        write(path, record)
    except Exception:
        record['status'] = 'MEASUREMENT_FAILED'
        write(path, record)
        raise
    finally:
        adb(args, 'shell', 'dumpsys deviceidle unforce')
        adb(args, 'shell', 'dumpsys battery reset')
    print('MEASURED, NOT CERTIFIED: finish the independent assertions listed in the record')


def self_test():
    endpoint = 'ab' * 32
    log = (f'2026-11-22T01:00:00.123Z INFO factoryd: tunnel connection opened endpoint={endpoint} compressed=true\n'
           f'2026-11-22T01:00:00.200Z INFO factoryd: device connected device=synthetic endpoint={endpoint}\n'
           f'2026-11-22T01:00:01Z INFO factoryd: tunnel connection opened endpoint={"cd" * 32}\n')
    assert len(endpoint_lines(log, endpoint, 'tunnel connection opened')) == 1
    assert timestamp(endpoint_lines(log, endpoint, 'device connected')[0]) > 0
    assert relay_counts('registering client remote_endpoint=ababababab\n'
                        'unregistering client remote_endpoint=ababababab\n'
                        'registering client remote_endpoint=cdcdcdcdcd', endpoint) == {
                            'registrations': 1, 'duplicates': 0}
    assert relay_counts('multiple connections found, deactivating old connection '
                        'remote_endpoint=ababababab', endpoint)['duplicates'] == 1
    assert relay_counts('registering client \x1b[3mremote_endpoint\x1b[0m\x1b[2m=\x1b[0mababababab\n'
                        'unregistering client remote_endpoint=ababababab\n'
                        'multiple connections found, deactivating old connection '
                        '\x1b[3mremote_endpoint\x1b[0m\x1b[2m=\x1b[0mababababab', endpoint) == {
                            'registrations': 1, 'duplicates': 1}
    assert idle('mState=IDLE mLightState=ACTIVE')
    assert not idle('mState=IDLE_MAINTENANCE')
    assert not idle('mLightState=IDLE mState=ACTIVE')
    print('PASS: synthetic endpoint scoping, UTC time, relay registration and deep-idle parsing')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--adb', default='adb')
    parser.add_argument('--serial', default='emulator-5554')
    parser.add_argument('--package', default='com.claktory.app.dev')
    parser.add_argument('--own-emulator-slot', action='store_true')
    sub = parser.add_subparsers(dest='command', required=True)
    sub.add_parser('self-test').set_defaults(func=lambda _: self_test())
    start = sub.add_parser('begin')
    start.add_argument('--record', type=Path, required=True)
    start.add_argument('--backend-log', type=Path, required=True)
    start.add_argument('--relay-log', type=Path)
    start.add_argument('--endpoint', required=True)
    start.add_argument('--transport', choices=['direct', 'relay'], required=True)
    start.add_argument('--seconds', type=int, choices=[300, 3600, 5400], required=True)
    start.add_argument('--connected-label', required=True)
    start.set_defaults(func=begin)
    finish = sub.add_parser('resume')
    finish.add_argument('--record', type=Path, required=True)
    finish.set_defaults(func=resume)
    args = parser.parse_args()
    try:
        args.func(args)
    except Refused as error:
        print('REFUSED: ' + str(error))
        return 1
    except Exception:
        print('FAILED: sensitive tool/log/UI output withheld')
        return 1
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
