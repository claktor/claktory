"""release_apk.sh runs under `set -o pipefail`: a reader that stops early
(grep -q, head) makes the command feeding it die of SIGPIPE once its
output outgrows the pipe, and the release fails on a good APK.

    python3 -m unittest discover -s app/android/tools
"""
import os
import re
import unittest

import check_artifact as ca

EARLY_EXIT = re.compile(r"\|\s*(grep\s+(-\w*q\w*|--quiet|--silent)\b|head\b)")


class ReleaseApk(unittest.TestCase):
    def test_no_pipe_into_a_reader_that_stops_early(self):
        with open(os.path.join(ca.ANDROID, "tools", "release_apk.sh"), encoding="utf-8") as f:
            script = f.read()
        self.assertIn("set -euo pipefail", script)
        found = [line.strip() for line in script.splitlines() if EARLY_EXIT.search(line)]
        self.assertEqual([], found)

    def test_the_pattern_sees_the_pipes_that_fail(self):
        for line in ('aapt2 dump resources "$apk" | grep -q x', "keytool -printcert | head -1",
                     "a | grep -qx b", "a |grep --quiet b"):
            self.assertTrue(EARLY_EXIT.search(line), line)
        for line in ("a | sed -n 1p", "a | grep x", "[[ $x == *y* ]]"):
            self.assertFalse(EARLY_EXIT.search(line), line)


if __name__ == "__main__":
    unittest.main()
