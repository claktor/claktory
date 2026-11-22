#!/usr/bin/env python3
"""Black-box helpers for the UNMODIFIED friend APK. See friend_apk_gate.md.

Never builds, re-signs, adb-installs, approves devices, or saves UI XML.
UI output can contain credentials: only assertion results reach evidence/logs.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import shlex
import subprocess
import sys
import time
from urllib.parse import parse_qs, urlparse
from urllib.request import Request, urlopen
import uuid
import xml.etree.ElementTree as ET

PACKAGE = "com.claktory.app"
CERT = "a4cf34938b823d9017eb01fe8b2e0978f481ec931dfd4fbfa1d705ddc34dd41e"
VERSION = 101502


class GateFailure(Exception):
    pass


def require(condition, message):
    if not condition:
        raise GateFailure(message)


def run(command):
    result = subprocess.run(command, capture_output=True)
    # Never expose a failed command or stdout: it could contain a code/UI XML.
    require(result.returncode == 0, "External tool failed (output withheld)")
    return result.stdout.decode("utf-8", errors="replace")


def digest(path):
    result = hashlib.sha256()
    with open(path, "rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            result.update(chunk)
    return result.hexdigest()


def hex_digest(value):
    value = value.lower().replace(":", "")
    require(bool(re.fullmatch(r"[0-9a-f]{64}", value)), "Need a full SHA-256")
    return value


def event(args, details):
    details = {"stage": args.stage, "result": "PASS", **details}
    args.evidence.mkdir(parents=True, exist_ok=True)
    path = args.evidence / "events.jsonl"
    with open(path, "a", encoding="utf-8") as output:
        output.write(json.dumps(details, sort_keys=True) + "\n")
    print(json.dumps(details, sort_keys=True))


def artifact(args):
    expected = hex_digest(args.sha256)
    parsed = urlparse(args.url)
    require(parsed.scheme == "https" or
            (parsed.scheme == "http" and parsed.hostname in
             ("127.0.0.1", "localhost", "10.0.2.2")),
            "Use HTTPS, or an explicitly local staging server")
    require(not parsed.username and not parsed.password, "No URL credentials")
    require(not args.apk.exists(), "Download destination already exists")
    args.apk.parent.mkdir(parents=True, exist_ok=True)
    try:
        # Public hosting may reject Python's default bot User-Agent while
        # allowing the real browser download. Keep TLS verification enabled.
        request = Request(args.url, headers={
            "User-Agent": "Mozilla/5.0 (compatible; Claktory-Android-Gate/1.0)"})
        with urlopen(request, timeout=60) as response, open(args.apk, "xb") as output:
            final = urlparse(response.url)
            require(final.scheme == "https" or
                    (final.scheme == "http" and final.hostname in
                     ("127.0.0.1", "localhost", "10.0.2.2")),
                    "Unsafe artifact redirect")
            size = 0
            for chunk in iter(lambda: response.read(1024 * 1024), b""):
                size += len(chunk)
                require(size <= 512 * 1024 * 1024, "APK exceeds download limit")
                output.write(chunk)
        require(digest(args.apk) == expected, "Downloaded APK hash mismatch")
        signed = run([args.apksigner, "verify", "--verbose", "--print-certs",
                      str(args.apk)])
        certificates = re.findall(r"Signer #\d+ certificate SHA-256 digest: (\S+)", signed)
        require(len(certificates) == 1 and hex_digest(certificates[0]) == CERT,
                "APK signer does not match the friend-build certificate")
        require("Verified using v2 scheme (APK Signature Scheme v2): true" in signed,
                "APK Signature Scheme v2 verification missing")
        manifest = run([args.aapt2, "dump", "badging", str(args.apk)])
        require(re.search(rf"package: name='com\.claktory\.app' versionCode='{args.version_code}' "
                          r"versionName='0\.1\.15'", manifest),
                "Wrong prod package or expected Android version")
        require("application-debuggable" not in manifest, "APK is debuggable")
        require("native-code: 'arm64-v8a' 'x86_64'" in manifest,
                "Expected both shipped 64-bit ABIs")
        require(digest(args.apk) == expected, "Artifact changed during verification")
    except Exception:
        # Keep the failed file for local diagnosis; never proceed with it.
        raise
    event(args, {"apk_sha256": expected, "certificate_sha256": CERT,
                 "version_code": args.version_code, "package": PACKAGE,
                 "source_commit_owner_reported": args.source_commit,
                 "download_host": parsed.hostname,
                 "signature_v2": True, "apk_modified": False})


def adb(args, *command):
    return run([args.adb, "-s", args.serial, *command])


def ui_tree(args):
    # Android's shell-private temporary file, mode 0600 before uiautomator
    # writes it. Always remove it, including failed dumps. No /sdcard XML,
    # screenshots, host UI files, or XML in an exception/log.
    path = "/data/local/tmp/claktory-gate-" + uuid.uuid4().hex + ".xml"
    command = (f"umask 077; trap 'rm -f {path}' EXIT; : > {path}; chmod 600 {path}; "
               f"uiautomator dump {path} >/dev/null && cat {path}")
    data = adb(args, "shell", command)
    start, end = data.find("<?xml"), data.rfind("</hierarchy>")
    require(start >= 0 and end >= 0, "Could not obtain Android semantics")
    try:
        return ET.fromstring(data[start:end + len("</hierarchy>")])
    except ET.ParseError:
        raise GateFailure("Invalid Android semantics XML") from None


def matches(tree, label, contains=False):
    def match(node):
        labels = (node.get("text", ""), node.get("content-desc", ""), node.get("hint", ""))
        return any(label in text if contains else label == text for text in labels)
    return [node for node in tree.iter("node") if match(node)]


def point(node):
    bounds = re.fullmatch(r"\[(\d+),(\d+)\]\[(\d+),(\d+)\]", node.get("bounds", ""))
    require(bounds is not None, "Missing control bounds")
    x1, y1, x2, y2 = map(int, bounds.groups())
    require(x2 > x1 and y2 > y1, "Control is not visible")
    return (x1 + x2) // 2, (y1 + y2) // 2


def foreground(args):
    state = adb(args, "shell", "dumpsys activity activities")
    # Both Android formats appear across API levels. Never accept an old
    # background activity merely because the package occurs elsewhere.
    resumed = [line for line in state.splitlines()
               if "topResumedActivity" in line or "mResumedActivity" in line]
    require(any(PACKAGE + "/" in line for line in resumed),
            "Prod Claktory is not the foreground activity")


def ui(args):
    deadline = time.monotonic() + args.timeout
    require(args.expect or args.absent or args.tap or args.disabled,
            "Specify a named surface assertion or action")
    while True:
        try:
            tree = ui_tree(args)
        except GateFailure as error:
            if str(error) != "Could not obtain Android semantics":
                raise
            require(time.monotonic() < deadline, "Android semantics stayed unavailable")
            time.sleep(0.25)
            continue
        present = all(matches(tree, label, args.contains) for label in args.expect)
        if args.tap:
            present = present and len(matches(tree, args.tap, args.contains)) == 1
        absent = not any(matches(tree, label, args.contains) for label in args.absent)
        disabled = all(len(matches(tree, label)) == 1 and
                       matches(tree, label)[0].get("enabled") == "false"
                       for label in args.disabled)
        if present and absent and disabled:
            break
        require(time.monotonic() < deadline, "Named surface assertion timed out")
        time.sleep(0.25)
    if args.app_foreground:
        foreground(args)
    if args.tap:
        found = matches(tree, args.tap, args.contains)
        require(len(found) == 1, "Tap requires exactly one named control")
        require(found[0].get("enabled") == "true", "Control is disabled")
        x, y = point(found[0])
        adb(args, "shell", f"input tap {x} {y}")
    # Record counts, not labels (a label could be a code or private message).
    event(args, {"present_assertions": len(args.expect),
                 "absent_assertions": len(args.absent),
                 "disabled_assertions": len(args.disabled), "tap": bool(args.tap),
                 "app_foreground": args.app_foreground})


def safe_code(raw, expected_endpoint, relay_only):
    parsed = urlparse(raw.strip())
    values = parse_qs(parsed.query, strict_parsing=True)
    require(parsed.scheme == "factory", "Not a tunnel pairing code")
    require(len(values.get("e", [])) == 1 and len(values.get("t", [])) == 1,
            "Pairing code must have one endpoint and token")
    endpoint, token = values["e"][0], values["t"][0]
    require(endpoint == expected_endpoint, "Unexpected backend endpoint")
    require(bool(re.fullmatch(r"[0-9a-f]{64}|[a-z2-7]{52}", endpoint)),
            "Invalid endpoint")
    require(bool(re.fullmatch(r"[0-9a-fA-F]{64}", token)), "Invalid code token")
    # Only the display name and direct-address hints change. Authentication
    # token/endpoint/relay remain those of the original privately supplied code.
    # ASCII, unescaped values avoid adb input text's percent handling. Do NOT
    # change the real app's parser or weaken identity checks for this test.
    parts = ["e=" + endpoint, "t=" + token, "name=Android-gate"]
    relays = values.get("r", [])
    require(not relay_only or relays, "Relay test needs explicit relay addresses")
    for relay in relays:
        uri = urlparse(relay)
        require(uri.scheme in ("http", "https") and uri.hostname and
                not uri.username and not uri.password and not uri.query and
                not uri.fragment and not re.search(r"[%\s&'\\]", relay),
                "Relay cannot be typed safely; use manual paste")
        parts.append("r=" + relay)
    if not relay_only:
        for address in values.get("a", []):
            require(bool(re.fullmatch(r"[0-9a-fA-F:.\[\]]+:\d{1,5}", address)),
                    "Invalid direct-address hint")
            parts.append("a=" + address)
    return "factory://?" + "&".join(parts)


def type_text(args):
    foreground(args)
    if args.code_file:
        require(args.backend_port == 7421 or 7500 <= args.backend_port <= 65535,
                "Only dev7421 or an isolated QA backend port is allowed")
        value = safe_code(args.code_file.read_text(), args.endpoint, args.relay_only)
    elif args.old_code:
        value = "factory://127.0.0.1:7500?t=" + "a" * 64 + "&fp=" + "b" * 64
    else:
        value = args.value
    require(value and not re.search(r"[%\s]", value),
            "This helper types only ASCII without spaces/percent escapes")
    require(value.isascii(), "Use manual paste for non-ASCII text")
    fields = [node for node in ui_tree(args).iter("node")
              if node.get("class") == "android.widget.EditText" and
              node.get("package") == PACKAGE]
    require(len(fields) == 1, "Need exactly one visible app input")
    x, y = point(fields[0])
    adb(args, "shell", f"input tap {x} {y}")
    # Ctrl+A from Android's keycombination is not consistently forwarded to
    # Flutter. Clear with real End/Delete keys, sized from in-memory text;
    # never log that text (it may be the previous private code).
    old_length = len(fields[0].get("text", ""))
    require(old_length <= 8192, "Input too long to clear safely")
    adb(args, "shell", "input keyevent KEYCODE_MOVE_END")
    if old_length:
        adb(args, "shell", "input keyevent " + " ".join(["KEYCODE_DEL"] * old_length))
    adb(args, "shell", "input text " + shlex.quote(value))
    # Deliberately no UI dump/event containing the entered value.
    event(args, {"typed": "code" if args.code_file else "synthetic",
                 "direct_hints_removed": args.relay_only})


def installed(args):
    expected = hex_digest(args.sha256)
    paths = adb(args, "shell", "pm path " + PACKAGE).splitlines()
    require(len(paths) == 1 and paths[0].startswith("package:"),
            "Expected one installed universal prod APK")
    path = paths[0][len("package:"):].strip()
    result = hashlib.sha256()
    process = subprocess.Popen([args.adb, "-s", args.serial, "exec-out", "cat", path],
                               stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    with process.stdout:
        for chunk in iter(lambda: process.stdout.read(1024 * 1024), b""):
            result.update(chunk)
    require(process.wait() == 0, "Cannot hash installed APK")
    require(result.hexdigest() == expected, "Installed APK is not the downloaded artifact")
    package = adb(args, "shell", "dumpsys package " + PACKAGE)
    require(re.search(rf"\bversionCode={args.version_code}\b", package), "Wrong installed versionCode")
    event(args, {"installed_apk_sha256": expected, "version_code": args.version_code,
                 "same_downloaded_signed_bytes": True})


def action(args):
    if args.command == "open-download":
        uri = urlparse(args.url)
        require(uri.scheme == "https" or
                (uri.scheme == "http" and uri.hostname == "10.0.2.2"),
                "Browser download requires HTTPS or emulator-host staging")
        adb(args, "shell", "am start -a android.intent.action.VIEW -d " + shlex.quote(args.url))
    elif args.command == "back":
        foreground(args)
        adb(args, "shell", "input keyevent KEYCODE_BACK")
    else:
        adb(args, "shell", "am force-stop " + PACKAGE)
        adb(args, "shell", "am start -n " + PACKAGE + "/com.claktory.app.MainActivity")
    event(args, {"action": args.command})


def self_test():
    tree = ET.fromstring('<hierarchy><node text="" content-desc="Paste code" '
                         'enabled="true" bounds="[1,2][49,50]"/></hierarchy>')
    assert len(matches(tree, "Paste code")) == 1
    assert not matches(tree, "Paste")
    assert len(matches(tree, "Paste", True)) == 1
    assert matches(ET.fromstring('<hierarchy><node hint="Paste code"/></hierarchy>'),
                   'Paste code')
    assert point(matches(tree, "Paste code")[0]) == (25, 26)
    endpoint = "a" * 64
    original = ("factory://?e=" + endpoint + "&t=" + "b" * 64 +
                "&name=Phone%E2%80%99s&r=https%3A%2F%2Frelay.claktory.com%2F&a=10.0.0.2:1234")
    normalized = safe_code(original, endpoint, True)
    assert parse_qs(urlparse(normalized).query)["t"] == ["b" * 64]
    assert "a" not in parse_qs(urlparse(normalized).query)
    assert "%" not in normalized
    assert parse_qs(urlparse(safe_code(original, endpoint, False)).query)["a"] == ["10.0.0.2:1234"]
    for bad in ("factory://?e=wrong&t=" + "b" * 64, "not-a-code"):
        try:
            safe_code(bad, endpoint, True)
            raise AssertionError("Bad code accepted")
        except GateFailure:
            pass
    try:
        point(ET.fromstring('<node bounds="[0,0][0,0]"/>'))
        raise AssertionError("Invisible control accepted")
    except GateFailure:
        pass
    print("PASS: synthetic UI, code normalization, identity rejection, invisible target")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evidence", type=Path, default=Path("app/build/android-friend/evidence"))
    parser.add_argument("--stage", default="unnamed")
    parser.add_argument("--version-code", type=int, default=VERSION)
    parser.add_argument("--adb", default="adb")
    parser.add_argument("--serial", default="emulator-5554")
    sub = parser.add_subparsers(dest="command", required=True)
    preflight = sub.add_parser("artifact")
    for name in ("url", "sha256", "source-commit", "apksigner", "aapt2"):
        preflight.add_argument("--" + name, required=True)
    preflight.add_argument("--apk", type=Path, required=True)
    preflight.set_defaults(func=artifact)
    check = sub.add_parser("ui")
    check.add_argument("--expect", action="append", default=[])
    check.add_argument("--absent", action="append", default=[])
    check.add_argument("--contains", action="store_true")
    check.add_argument("--disabled", action="append", default=[])
    check.add_argument("--tap")
    check.add_argument("--app-foreground", action="store_true")
    check.add_argument("--timeout", type=float, default=30)
    check.set_defaults(func=ui)
    typing = sub.add_parser("type")
    source = typing.add_mutually_exclusive_group(required=True)
    source.add_argument("--value")
    source.add_argument("--old-code", action="store_true")
    source.add_argument("--code-file", type=Path)
    typing.add_argument("--endpoint", default="")
    typing.add_argument("--backend-port", type=int, default=0)
    typing.add_argument("--relay-only", action="store_true")
    typing.set_defaults(func=type_text)
    verify = sub.add_parser("installed")
    verify.add_argument("--sha256", required=True)
    verify.set_defaults(func=installed)
    for command in ("open-download", "back", "relaunch"):
        command_parser = sub.add_parser(command)
        if command == "open-download":
            command_parser.add_argument("--url", required=True)
        command_parser.set_defaults(func=action)
    sub.add_parser("self-test").set_defaults(func=lambda _: self_test())
    args = parser.parse_args()
    try:
        require(101500 <= args.version_code <= 101599, "Expected a v0.1.15 build number")
        args.func(args)
    except GateFailure as error:
        # GateFailure messages are constant strings, never raw UI/commands.
        print("FAIL: " + str(error), file=sys.stderr)
        return 1
    except Exception:
        # No traceback, command echo, URL/code, UI dump or parsed values.
        print("FAIL: gate operation failed; inspect locally without sharing credentials", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
