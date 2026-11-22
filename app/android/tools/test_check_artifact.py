"""The artifact verifier refuses each way an Android build can fail on a
phone, and passes a good one.

    python3 -m unittest discover -s app/android/tools

No Android SDK needed: the artifacts are synthetic (fixtures.py).
"""
import contextlib
import io
import os
import shutil
import subprocess
import tempfile
import unittest

import check_artifact as ca
import fixtures as fx
from check_artifact import EM_ARM, EM_X86_64


def policy(release=True):
    toolchain = ca.read_toolchain(os.path.join(ca.ANDROID, "toolchain.properties"))
    exports = ca.FRB_CORE_EXPORTS + ca.generated_exports(
        os.path.join(ca.APP, "lib", "tunnel", "frb_generated.io.dart"))
    return ca.Policy(toolchain, exports, release=release)


def lib(name="libclaktory_tunnel.so", **kwargs):
    """One library checked alone, in an arm64-v8a folder with good neighbours."""
    if name == "libclaktory_tunnel.so":
        kwargs.setdefault("exports", fx.tunnel_exports())
    data = fx.build_so(**kwargs)
    return ca.check_elf(f"lib/arm64-v8a/{name}", data, policy(), abi="arm64-v8a",
                        siblings={"libclaktory_tunnel.so", "libflutter.so", "libapp.so"})


class Library(unittest.TestCase):
    def assertRefused(self, result, words):
        problems, _ = result
        self.assertTrue(any(words in p for p in problems), f"wanted {words!r} in {problems}")

    def test_a_good_tunnel_passes(self):
        self.assertEqual(lib(), ([], []))

    def test_4k_load_alignment_is_refused(self):
        self.assertRefused(lib(align=4096), "aligned to 4096 bytes, not 16384")

    def test_relro_ending_mid_page_before_writable_data_is_refused(self):
        self.assertRefused(lib(relro_page=4096), "RELRO ends at")

    def test_wrong_machine_for_the_folder_is_refused(self):
        self.assertRefused(lib(machine=EM_X86_64), "but its folder is arm64-v8a")

    def test_newer_api_than_min_sdk_is_refused(self):
        self.assertRefused(lib(api=35), "linked for Android API 35, newer than the app's minSdk 24")

    def test_missing_api_note_is_refused_for_ndk_libraries(self):
        self.assertRefused(lib(note=False), "no Android API note")

    def test_dart_code_needs_no_api_note(self):
        self.assertEqual(lib("libapp.so", note=False, needed=()), ([], []))

    def test_an_unshipped_dependency_is_refused(self):
        self.assertRefused(lib(needed=("libc.so", "libc++_shared.so")), "needs libc++_shared.so")

    def test_a_missing_frb_export_is_refused(self):
        exports = [e for e in fx.tunnel_exports() if e != "frb_pde_ffi_dispatcher_primary"]
        self.assertRefused(lib(exports=exports), "doesn't export frb_pde_ffi_dispatcher_primary")

    def test_a_missing_bootstrap_export_is_refused(self):
        exports = [e for e in fx.tunnel_exports() if not e.endswith("nativeInstall")]
        self.assertRefused(lib(exports=exports), "doesn't export Java_com_claktory_app_TunnelBootstrap_nativeInstall")

    def test_an_import_doesnt_count_as_an_export(self):
        elf = ca.Elf(fx.build_so(exports=["a"]))
        self.assertEqual(elf.exports(), {"a"})

    def test_shipped_debug_info_is_refused(self):
        self.assertRefused(lib(debug=True), "ships debug info")

    def test_an_executable_is_refused(self):
        self.assertRefused(lib(elf_type=2), "not a shared object")

    def test_a_home_folder_path_is_refused(self):
        self.assertRefused(lib(home_path=True), "names a home folder (/Users/someone/)")

    def test_flutters_own_strings_dont_count_as_home_folders(self):
        # Flutter's debug engine holds "/Users/myself/" in its own text.
        self.assertEqual(lib("libflutter.so", home_path=True), ([], []))

    def test_not_an_elf_is_refused(self):
        problems, _ = ca.check_elf("x.so", b"MZ not elf", policy())
        self.assertEqual(problems, ["x.so: not an ELF file"])


class Apk(unittest.TestCase):
    def check(self, blob, release=True):
        problems, _, _ = ca.check_apk("app.apk", blob, policy(release))
        return problems

    def test_a_good_apk_passes(self):
        self.assertEqual(self.check(fx.good_apk(abis=("arm64-v8a", "x86_64"))), [])

    def test_a_debug_apk_without_dart_code_passes_as_debug(self):
        self.assertEqual(self.check(fx.good_apk(libs=fx.good_libs(release=False)), release=False), [])

    def test_misaligned_library_is_refused(self):
        entries = [(f"lib/arm64-v8a/{n}", d, True, n != "libclaktory_tunnel.so")
                   for n, d in fx.good_libs().items()]
        problems = self.check(fx.build_zip(entries))
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("libclaktory_tunnel.so: starts at", problems[0])

    def test_compressed_library_is_refused(self):
        entries = [(f"lib/arm64-v8a/{n}", d, n != "libflutter.so", True)
                   for n, d in fx.good_libs().items()]
        problems = self.check(fx.build_zip(entries))
        self.assertTrue(any("libflutter.so: compressed" in p for p in problems), problems)

    def test_an_abi_folder_without_the_tunnel_is_refused(self):
        libs = fx.good_libs()
        del libs["libclaktory_tunnel.so"]
        problems = self.check(fx.good_apk(libs=libs))
        self.assertIn("app.apk: lib/arm64-v8a/ has no libclaktory_tunnel.so", problems)

    def test_an_abi_the_app_doesnt_ship_is_refused(self):
        libs = {n: fx.build_so(machine=EM_ARM, exports=fx.tunnel_exports()) for n in fx.good_libs()}
        problems = self.check(fx.good_apk(abis=("armeabi-v7a",), libs=libs))
        self.assertTrue(any("ABI armeabi-v7a isn't one the app ships" in p for p in problems), problems)

    def test_no_native_libraries_is_refused(self):
        blob = fx.build_zip([("AndroidManifest.xml", b"<manifest/>", False, False)])
        self.assertEqual(self.check(blob), ["app.apk: no native libraries at all (the tunnel isn't in it)"])

    def test_dependency_on_a_shipped_neighbour_passes(self):
        libs = fx.good_libs()
        libs["libclaktory_tunnel.so"] = fx.build_so(needed=("libc.so", "libc++_shared.so"),
                                                     exports=fx.tunnel_exports())
        libs["libc++_shared.so"] = fx.build_so(needed=("libc.so",))
        self.assertEqual(self.check(fx.good_apk(libs=libs)), [])


class Bundle(unittest.TestCase):
    def test_a_bundle_without_bundletool_fails_closed(self):
        entries = [(f"base/lib/arm64-v8a/{n}", d, False, False) for n, d in fx.good_libs().items()]
        blob = fx.build_zip(entries)
        problems, _ = ca.check_aab("app.aab", "app.aab", blob, policy(), bundletool=None)
        self.assertEqual(problems, ["app.aab: its 16 KiB setting is unchecked (give --bundletool JAR)"])

    def test_split_apks_are_checked_one_by_one(self):
        base = fx.build_zip([("AndroidManifest.xml", b"<manifest/>", False, False)])
        bad = fx.build_zip([(f"lib/arm64-v8a/{n}", fx.build_so(align=4096, exports=fx.tunnel_exports()), True, True)
                            for n in fx.good_libs()])
        blob = fx.build_zip([("splits/base-master.apk", base, False, False),
                             ("splits/base-arm64_v8a.apk", bad, False, False)])
        problems, _ = ca.check_apks("app.apks", blob, policy())
        self.assertTrue(problems)
        self.assertTrue(all("splits/base-arm64_v8a.apk!lib/arm64-v8a/" in p for p in problems), problems)


def run(argv):
    with contextlib.redirect_stdout(io.StringIO()):
        return ca.main(argv)


class CommandLine(unittest.TestCase):
    def test_exit_status_and_folder_search(self):
        with tempfile.TemporaryDirectory() as tmp:
            with open(os.path.join(tmp, "good.apk"), "wb") as f:
                f.write(fx.good_apk())
            self.assertEqual(run([tmp]), 0)
            with open(os.path.join(tmp, "bad.apk"), "wb") as f:
                f.write(fx.good_apk(libs={n: fx.build_so(align=4096) for n in fx.good_libs()}))
            self.assertEqual(run([tmp]), 1)

    def test_nothing_to_check_fails(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.assertEqual(run([tmp]), 1)


@unittest.skipUnless(shutil.which("objdump"), "no llvm-objdump")
class FixturesAreRealElf(unittest.TestCase):
    """LLVM reads the fixtures as the verifier does, so the verifier isn't
    only agreeing with its own writer."""

    def test_llvm_objdump_agrees(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "libx.so")
            with open(path, "wb") as f:
                f.write(fx.build_so(exports=["frb_get_rust_content_hash"], needed=("libc.so", "libm.so"),
                                    align=4096))
            out = subprocess.run(["objdump", "-p", "-T", path], capture_output=True, text=True)
            if "elf64-littleaarch64" not in out.stdout:
                self.skipTest("this objdump doesn't read ELF")
            self.assertIn("align 2**12", out.stdout)
            self.assertIn("NEEDED libc.so", out.stdout)
            self.assertIn("NEEDED libm.so", out.stdout)
            self.assertRegex(out.stdout, r"g\s+DF \.text\s+\w+ frb_get_rust_content_hash")
            self.assertRegex(out.stdout, r"DF \*UND\*\s+\w+ malloc")


def engine_library():
    """Flutter's own release libflutter.so for arm64, from the engine cache
    of the flutter on PATH, or None."""
    flutter = shutil.which("flutter")
    if not flutter:
        return None
    root = os.path.dirname(os.path.dirname(os.path.realpath(flutter)))
    jar = os.path.join(root, "bin", "cache", "artifacts", "engine", "android-arm64-release", "flutter.jar")
    if not os.path.exists(jar):
        return None
    import zipfile
    with zipfile.ZipFile(jar) as z:
        return z.read("lib/arm64-v8a/libflutter.so")


class RealEngine(unittest.TestCase):
    """The rules hold on a real Android library: Flutter's engine, as the
    pinned Flutter ships it (before the Android build strips it)."""

    def test_flutters_engine_passes_but_for_its_debug_info(self):
        data = engine_library()
        if data is None:
            self.skipTest("no Flutter engine cache")
        problems, _ = ca.check_elf("lib/arm64-v8a/libflutter.so", data, policy(), abi="arm64-v8a",
                                   siblings=set())
        # Its RELRO doesn't end on 16 KiB, yet nothing writable shares that
        # page: a rule demanding an aligned end would refuse Flutter itself.
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("ships debug info", problems[0])


if __name__ == "__main__":
    unittest.main()
