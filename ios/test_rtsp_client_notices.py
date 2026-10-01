"""Tests for the license expression check in rtsp-client-notices.py.

Run with: python3 ios/test_rtsp_client_notices.py
"""

import importlib.util
import unittest
from pathlib import Path

SPEC = importlib.util.spec_from_file_location("notices", Path(__file__).with_name("rtsp-client-notices.py"))
notices = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(notices)


class PermissiveTests(unittest.TestCase):
    def test_accepts_allowlisted_expressions(self):
        for expression in [
            "MIT",
            "MIT OR Apache-2.0",
            "MIT/Apache-2.0",
            "Apache-2.0 OR MIT",
            "Unlicense OR MIT",
            "Apache-2.0 WITH LLVM-exception",
            "Apache-2.0 OR Apache-2.0 WITH LLVM-exception OR MIT",
            "(MIT OR Apache-2.0) AND Unicode-3.0",
            "Zlib OR Apache-2.0 OR MIT",
            "GPL-3.0-only OR MIT",
        ]:
            with self.subTest(expression=expression):
                self.assertTrue(notices.permissive(expression))

    def test_rejects_licenses_outside_the_allowlist(self):
        for expression in [
            "GPL-3.0-only",
            "MIT AND GPL-3.0-only",
            "(MIT OR Apache-2.0) AND LGPL-2.1-or-later",
            "MIT WITH LLVM-exception",
        ]:
            with self.subTest(expression=expression):
                self.assertFalse(notices.permissive(expression))

    def test_rejects_malformed_expressions(self):
        for expression in [
            None,
            "",
            "MIT GPL-3.0-only",
            "MIT) AND GPL-3.0-only",
            "(MIT OR Apache-2.0",
            "MIT OR Apache-2.0)",
            "((MIT)",
            "MIT OR",
            "OR MIT",
            "MIT AND",
            "Apache-2.0 WITH",
            "MIT OR AND Apache-2.0",
            "()",
        ]:
            with self.subTest(expression=expression):
                self.assertFalse(notices.permissive(expression))


if __name__ == "__main__":
    unittest.main()
