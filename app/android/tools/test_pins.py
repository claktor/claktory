"""The Gradle files build with exactly the pinned tuple
(toolchain.properties): where a version has to be a literal, it's the same
literal; elsewhere the build reads the pin.

    python3 -m unittest discover -s app/android/tools
"""
import os
import re
import unittest

import check_artifact as ca

ANDROID = ca.ANDROID


def read(*parts):
    with open(os.path.join(ANDROID, *parts), encoding="utf-8") as f:
        return f.read()


PINS = ca.read_toolchain(os.path.join(ANDROID, "toolchain.properties"))


class Pins(unittest.TestCase):
    def test_plugins_are_the_pinned_versions(self):
        settings = read("settings.gradle.kts")
        self.assertIn(f'id("com.android.application") version "{PINS["agp.version"]}"', settings)
        self.assertIn(f'id("org.jetbrains.kotlin.android") version "{PINS["kotlin.version"]}"', settings)

    def test_gradle_is_the_pinned_distribution_checked_by_its_hash(self):
        wrapper = read("gradle", "wrapper", "gradle-wrapper.properties")
        self.assertIn(f"gradle-{PINS['gradle.version']}-bin.zip", wrapper)
        self.assertIn(f"distributionSha256Sum={PINS['gradle.sha256']}", wrapper)

    def test_the_app_reads_its_levels_ndk_and_abis_from_the_pins(self):
        app = read("app", "build.gradle.kts")
        for key in ("android.compileSdk", "android.buildTools", "android.ndk", "android.minSdk",
                    "android.targetSdk", "android.abis"):
            self.assertIn(f'pin("{key}")', app, key)
        # No level written in by hand beside them.
        self.assertNotRegex(app, r"(minSdk|targetSdk|compileSdk)\s*=\s*\d")
        self.assertNotIn("flutter.ndkVersion", app)

    def test_the_build_never_installs_anything_itself(self):
        props = read("gradle.properties")
        self.assertIn("android.builder.sdkDownload=false", props)
        self.assertIn("org.gradle.java.installations.auto-download=false", props)

    def test_no_release_is_silently_debug_signed(self):
        app = read("app", "build.gradle.kts")
        release = app[app.index("buildTypes"):app.index("packaging")]
        self.assertIn('debugSignedRelease -> signingConfigs.getByName("debug")', release)
        self.assertIn("else -> null", release)

    def test_nothing_is_backed_up_or_transferred(self):
        manifest = read("app", "src", "main", "AndroidManifest.xml")
        self.assertIn('android:allowBackup="false"', manifest)
        self.assertIn('android:fullBackupContent="@xml/backup_rules"', manifest)
        self.assertIn('android:dataExtractionRules="@xml/data_extraction_rules"', manifest)
        rules = read("app", "src", "main", "res", "xml", "data_extraction_rules.xml")
        for section in ("cloud-backup", "device-transfer"):
            body = re.search(rf"<{section}>(.*?)</{section}>", rules, re.S).group(1)
            for domain in ("root", "file", "database", "sharedpref", "external"):
                self.assertIn(f'<exclude domain="{domain}" path="." />', body, (section, domain))

    def test_r8_keeps_the_bootstrap(self):
        self.assertIn("-keep class com.claktory.app.TunnelBootstrap", read("app", "proguard-rules.pro"))
        self.assertIn("Java_com_claktory_app_TunnelBootstrap_nativeInstall", ca.BOOTSTRAP_EXPORTS)


if __name__ == "__main__":
    unittest.main()
