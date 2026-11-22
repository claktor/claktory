#!/usr/bin/env python3
"""Real Android UI checks against an owned, guarded QA sandbox.

This measures named paths, not overall Android acceptance. A sandbox must
already be up from the app's recorded source (qa.sh --rev REV up ... --model).
No fake channels, lifecycle callbacks, real accounts or notification setup.
Needs system python3 (standard library only; no pip packages), adb, and the
repository's qa.sh. Reserve the emulator slot first. A recorded APK build must
supply source (full commit), apk (owned build path), sha256, flavor=dev and
debug=true. Run qa.sh --rev "$REV" up "$SANDBOX" --model first, then install
that APK. Example (all device/case values are supplied by the caller):

  python3 app/integration_test/android/native_core_gate.py \\
    --own-emulator-slot --adb "$ADB" --serial "$EMULATOR" \\
    --build-receipt "$RECEIPT" --qa-root "$QA_ROOT" --sandbox "$SANDBOX" \\
    --out "$OUT" --label "$LABEL" --agent "$AGENT" prepare

Use the same arguments with `upload --file "$FIXTURE" --nonce "$NONCE"`
afterwards. Prepare proves the actual paste/confirmation/number/reply path,
not QR scanning. Upload proves a real system document-picker selection, native
copy, attachment chip, Send, exact backend file bytes and linked visible reply.
It does not certify release builds, other-UID URI grants, physical devices,
notifications, all native features, or reconnect/Doze. Never run during a timed
Doze interval. Outputs stay under ignored app/build; archive sanitized results
outside build output before ending a case. Never print pairing material/UI.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import time
from types import SimpleNamespace
from urllib.parse import parse_qs, urlparse

ROOT = Path(__file__).resolve().parents[3]
PACKAGE = "com.claktory.app.dev"


def require(condition, message):
    if not condition:
        raise Failure(message)


class Failure(Exception):
    pass


def qa_port(port):
    require(isinstance(port, int) and not isinstance(port, bool)
            and 7500 <= port <= 65535 and port not in (7420, 7421),
            "Only a guarded localhost QA port is allowed")
    return port


def revision(value):
    require(isinstance(value, str) and re.fullmatch(r"[0-9a-f]{10,40}", value), "Recorded QA revision required")
    result = subprocess.run(["git", "rev-parse", "--verify", value + "^{commit}"], cwd=ROOT, capture_output=True)
    require(result.returncode == 0, "Recorded QA revision does not resolve")
    return result.stdout.decode().strip()


def owned(path):
    path = Path(path).resolve()
    require(path.is_relative_to((ROOT / "app/build").resolve()), "Owned build path required")
    return path


def save(path, value):
    path = owned(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as output:
        json.dump(value, output, indent=2)


class Gate:
    def __init__(self, args):
        require(args.own_emulator_slot, "Explicit emulator-slot ownership required")
        require(re.fullmatch(r"emulator-[0-9]+", args.serial), "Only a local emulator serial is allowed")
        require(not os.environ.get("ADB_SERVER_SOCKET") and not os.environ.get("ANDROID_ADB_SERVER_ADDRESS"),
                "Remote adb servers are not allowed")
        self.args = args
        self.out = owned(args.out)
        self.out.mkdir(parents=True, exist_ok=True)
        self.qa_root = owned(args.qa_root)
        self.build = json.loads(owned(args.build_receipt).read_text())
        self.rev = self.build["source"]
        require(re.fullmatch(r"[0-9a-f]{40}", self.rev), "Full source revision required")
        require(revision((self.qa_root / "bin" / self.rev[:10] / "commit").read_text().strip()) == self.rev,
                "Matching guarded QA commit marker required")
        require(self.build.get("flavor") == "dev" and self.build.get("debug") is True,
                "Only recorded dev debug builds")
        require(re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_-]{0,63}", args.sandbox), "Simple QA sandbox name required")
        sandbox = json.loads((self.qa_root / "sandboxes" / args.sandbox / "sandbox.json").read_text())
        require(revision(sandbox.get("commit")) == self.rev, "Running QA sandbox must match recorded app source")
        require(sandbox.get("name") == args.sandbox
                and owned(sandbox["bin"]) == self.qa_root / "bin" / self.rev[:10],
                "QA sandbox name or binary directory mismatch")
        qa_port(sandbox.get("port"))
        self.apk = owned(self.build["apk"])
        require(hashlib.sha256(self.apk.read_bytes()).hexdigest() == self.build["sha256"],
                "Recorded APK bytes changed")
        spec = importlib.util.spec_from_file_location("android_real_ui", ROOT / "app/integration_test/android/friend_apk_gate.py")
        self.ui_gate = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.ui_gate)
        self.ui_gate.PACKAGE = PACKAGE  # Local helper instance, never the friend APK gate's certification.
        self.u = SimpleNamespace(adb=args.adb, serial=args.serial, evidence=self.out / "events",
                                 stage="android-native-core", expect=[], absent=[], disabled=[],
                                 contains=False, tap=None, app_foreground=True, timeout=35)
        require(self.adb("shell", "getprop ro.kernel.qemu").strip() == "1", "Only the agreed emulator")
        self.env = os.environ.copy()
        self.env["CLAKTORY_QA_ROOT"] = str(self.qa_root)

    def adb(self, *args):
        return self.ui_gate.run([self.args.adb, "-s", self.args.serial, *args])

    def qa(self, *args):
        p = subprocess.run([str(ROOT / "scripts/qa.sh"), "--rev", self.rev, *args],
                           env=self.env, capture_output=True)
        require(p.returncode == 0, "Guarded QA request failed (details withheld)")
        return json.loads(p.stdout)

    def value(self, path):
        return self.qa("workspace", self.args.sandbox, path)["value"]

    def tree(self):
        return self.ui_gate.ui_tree(self.u)

    def ui(self, expect=(), absent=(), tap=None, contains=False, foreground=True):
        v = SimpleNamespace(**vars(self.u))
        v.expect, v.absent, v.tap = list(expect), list(absent), tap
        v.contains, v.app_foreground = contains, foreground
        self.ui_gate.ui(v)

    def type(self, text):
        require(text.isascii() and not re.search(r"[%\s]", text), "ASCII key-entry fixture required")
        fields = [n for n in self.tree().iter("node") if n.get("class") == "android.widget.EditText"
                  and n.get("package") == PACKAGE]
        require(len(fields) == 1 and not fields[0].get("text", ""), "One empty actual input required")
        x, y = self.ui_gate.point(fields[0])
        self.adb("shell", f"input tap {x} {y}")
        until = time.monotonic() + 10
        while "mInputShown=true" not in self.adb("shell", "dumpsys input_method"):
            require(time.monotonic() < until, "Actual IME not reached")
            time.sleep(.1)
        self.tree(); self.tree()  # Settle IME and its input connection before a private code.
        for start in range(0, len(text), 8):
            self.adb("shell", "input text " + shlex.quote(text[start:start + 8]))
            time.sleep(.04)
        fields = [n for n in self.tree().iter("node") if n.get("class") == "android.widget.EditText"
                  and n.get("package") == PACKAGE]
        require(len(fields) == 1 and fields[0].get("text") == text, "Full actual input readback mismatch")

    def installed(self):
        paths = self.adb("shell", "pm path " + PACKAGE).splitlines()
        require(len(paths) == 1 and paths[0].startswith("package:"), "One installed dev APK required")
        p = subprocess.run([self.args.adb, "-s", self.args.serial, "exec-out", "cat", paths[0][8:]], capture_output=True)
        require(p.returncode == 0 and hashlib.sha256(p.stdout).hexdigest() == self.build["sha256"],
                "Installed bytes differ from source receipt")

    def conversation(self):
        self.ui(expect=["Menu"])
        self.ui(tap="Menu")
        self.ui(expect=["Search", "Close menu"])
        rows = [n for n in self.tree().iter("node") if n.get("content-desc", "").startswith(self.args.agent)]
        require(len(rows) == 1, "Unique actual agent row required")
        self.ui(tap=rows[0].get("content-desc"))
        self.ui(expect=["Message " + self.args.agent], absent=["Close menu"])

    def roundtrip(self, nonce):
        require(1 <= len(nonce) <= 100 and re.fullmatch(r"[A-Za-z0-9_-]+", nonce), "Short synthetic nonce required")
        self.type(nonce)
        self.ui(tap="Send")
        self.ui(expect=["OK: " + nonce], contains=True)
        entries = self.qa("transcript", self.args.sandbox, self.args.agent, "--last", "80")["entries"]
        users = [e for e in entries if e.get("user", {}).get("text") == nonce]
        require(len(users) == 1, "Exactly one backend nonce required")
        require(any(e.get("assistant", {}).get("text") == "OK: " + nonce
                    and e.get("replied_to") == users[0]["seq"] for e in entries), "Exact backend reply link required")
        return users[0]

    def prepare(self):
        self.installed()
        self.qa("send", self.args.sandbox, "UpdateSettings", json.dumps({"user_name": "QA", "name": self.args.label}))
        agents = [a for a in self.value("agents") if a.get("name") == self.args.agent]
        require(len(agents) <= 1, "QA fixture has ambiguous agent names")
        if not agents:
            self.qa("send", self.args.sandbox, "CreateAgent", json.dumps({"name": self.args.agent}))
        pairing = self.qa("cli", self.args.sandbox, "pair", "Android-native-QA", "--no-wait", "--json")
        for _ in range(3):
            if parse_qs(urlparse(pairing["code"]).query).get("a"):
                break
            old = pairing
            pairing = self.qa("cli", self.args.sandbox, "pair", "Android-native-QA", "--no-wait", "--json")
            self.qa("cli", self.args.sandbox, "unpair", old["device_id"], "--json")
        require(bool(parse_qs(urlparse(pairing["code"]).query).get("a")), "Direct fixture lacks reachable hints")
        endpoint = self.value("remote.endpoint_id")
        self.adb("shell", "am force-stop " + PACKAGE)
        self.adb("shell", "pm clear " + PACKAGE)  # Fresh disposable dev fixture only; never during Doze.
        self.adb("shell", "am start -n " + PACKAGE + "/com.claktory.app.MainActivity")
        self.ui(tap="Paste code instead")
        # Canonical ASCII code: same token/endpoint/relay/hints, synthetic
        # display name only, avoiding adb's percent-escape interpretation.
        self.type(self.ui_gate.safe_code(pairing["code"], endpoint, False))
        self.adb("shell", "input keyevent KEYCODE_BACK")
        self.ui(tap="Connect")
        self.ui(expect=["Connect to this Claktory?"])
        tree = self.tree()
        fingerprint = " ".join(endpoint[i:i + 4] for i in range(0, 16, 4))
        require(bool(self.ui_gate.matches(tree, fingerprint, True)), "Actual fingerprint mismatch")
        require(bool(self.ui_gate.matches(tree, "This Claktory isn't on your Claktory account.", True)), "Off-account warning missing")
        require(not self.value("remote.pending_devices"), "No connection may precede confirmation")
        self.ui(tap="Connect")
        until = time.monotonic() + 45
        while True:
            pending = self.value("remote.pending_devices")
            numbers = [n.get("content-desc", "") for n in self.tree().iter("node")
                       if re.fullmatch(r"Number \d \d \d \d", n.get("content-desc", ""))]
            if pending and numbers:
                shown = "".join(re.findall(r"\d", numbers[0]))
                require(len(pending) == 1 and pending[0]["number"] == shown, "Actual number mismatch")
                self.qa("cli", self.args.sandbox, "allow", "Android-native-QA", "--number", shown, "--json")
                break
            require(time.monotonic() < until, "Number approval not reached")
            time.sleep(.15)
        self.conversation()
        self.roundtrip("postgso-native-pre")
        save(self.out / "prepare.json", {"source": self.rev, "result": "PASS", "path": "actual paste/fingerprint/number/nonce",
                                         "qr_scanned": False, "no_pending": not self.value("remote.pending_devices")})
        print("PASS: real paste, fingerprint confirmation, matching number approval and linked reply")

    def upload(self):
        self.installed()
        source = owned(self.args.file)
        require(source.is_file() and re.fullmatch(r"[A-Za-z0-9_.-]+", source.name), "Named owned fixture required")
        self.adb("push", str(source), "/sdcard/Download/" + source.name)
        if "mInputShown=true" in self.adb("shell", "dumpsys input_method"):
            self.adb("shell", "input keyevent KEYCODE_BACK")
            self.tree(); self.tree()
        self.ui(tap="Attach files")
        self.ui(expect=["Choose files"])
        self.ui(tap="Choose files")
        self.ui(tap="Search", foreground=False)
        self.adb("shell", "input text " + source.name)
        self.adb("shell", "input keyevent KEYCODE_ENTER")
        until = time.monotonic() + 15
        while True:
            titles = [n for n in self.tree().iter("node") if n.get("text") == source.name
                      and n.get("resource-id") == "android:id/title"]
            if len(titles) == 1:
                break
            require(time.monotonic() < until, "Actual document search title not reached")
        x, y = self.ui_gate.point(titles[0])
        self.adb("shell", f"input tap {x} {y}")
        self.ui(expect=[source.name], contains=True)
        entry = self.roundtrip(self.args.nonce)
        attachments = entry["user"]["attachments"]
        require(len(attachments) == 1, "Exactly one uploaded file required")
        target = Path(attachments[0]["path"]).resolve()
        require(target.is_relative_to((self.qa_root / "sandboxes" / self.args.sandbox).resolve()), "Uploaded file outside owned sandbox")
        require(source.read_bytes() == target.read_bytes(), "Backend bytes differ")
        result = {"source": self.rev, "result": "PASS", "path": "real document picker/copy/chip/Send",
                  "bytes": source.stat().st_size, "fixture_sha256": hashlib.sha256(source.read_bytes()).hexdigest(),
                  "nonce": self.args.nonce, "user_seq": entry["seq"], "visible_reply_and_replied_to": True}
        save(self.out / (self.args.nonce + ".json"), result)
        print("PASS: actual document picker to byte-exact backend file and linked reply")


def self_test():
    for port in (7500, 65535):
        assert qa_port(port) == port
    for port in (7420, 7421, 7499, 65536, -1, True, None, "7500"):
        try:
            qa_port(port)
        except Failure:
            pass
        else:
            raise AssertionError("Unsafe QA port accepted")
    assert owned(ROOT / "app/build/native-core-test") == (ROOT / "app/build/native-core-test").resolve()
    for path in (ROOT, ROOT / "app/build/../../lib", ROOT / "app/build/../test"):
        try:
            owned(path)
        except Failure:
            pass
        else:
            raise AssertionError("Unowned path accepted")
    print("PASS: local QA port and owned output path refusal checks (not OS E2E)")
    return 0


def main():
    import sys
    if sys.argv[1:] == ["self-test"]:
        return self_test()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--own-emulator-slot", action="store_true")
    parser.add_argument("--adb", required=True)
    parser.add_argument("--serial", required=True)
    parser.add_argument("--build-receipt", required=True)
    parser.add_argument("--qa-root", required=True)
    parser.add_argument("--sandbox", required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--label", required=True)
    parser.add_argument("--agent", required=True)
    subs = parser.add_subparsers(dest="command", required=True)
    subs.add_parser("prepare")
    upload = subs.add_parser("upload")
    upload.add_argument("--file", required=True)
    upload.add_argument("--nonce", required=True)
    args = parser.parse_args()
    try:
        gate = Gate(args)
        getattr(gate, args.command)()
    except (Failure, OSError, KeyError, ValueError) as error:
        print("FAILED: " + (str(error) if isinstance(error, Failure) else type(error).__name__) + " (private details withheld)")
        return 1
    except Exception as error:
        print("FAILED: " + type(error).__name__ + " (private details withheld)")
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
