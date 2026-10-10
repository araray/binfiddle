"""Transport contract tests for the bounded, optional GLM acquisition example.

Run separately from Cargo: python3 -m unittest discover -s tests -p 'test_glm53*.py'
No network calls or model payloads are needed.
"""

import importlib.util
import pathlib
import subprocess
import tempfile
import unittest
from unittest.mock import patch


SPEC = importlib.util.spec_from_file_location(
    "glm_sample", pathlib.Path(__file__).resolve().parents[1] / "examples/glm53_expert_sample.py")
SAMPLE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SAMPLE)


class DownloadContract(unittest.TestCase):
    def response(self, status, headers, body):
        def fake_curl(command, **kwargs):
            self.assertIn("--max-filesize", command)
            self.assertIn("--max-time", command)
            pathlib.Path(command[command.index("--dump-header") + 1]).write_text(
                "HTTP/1.1 302 Found\nLocation: https://example.invalid/shard\n\n"
                f"HTTP/2 {status}\n{headers}\n\n")
            pathlib.Path(command[command.index("--output") + 1]).write_bytes(body)
            return subprocess.CompletedProcess(command, 0)
        return fake_curl

    def test_accepts_exact_range_after_redirect(self):
        with tempfile.TemporaryDirectory() as scratch, patch.object(
            SAMPLE.subprocess, "run", self.response(206, "Content-Range: bytes 10-13/100\nContent-Length: 4", b"abcd")
        ):
            self.assertEqual(SAMPLE.fetch_once("shard", 4, (10, 14), scratch=scratch), (b"abcd", 100))
            self.assertEqual(list(pathlib.Path(scratch).iterdir()), [])

    def test_rejects_ignored_range_wrong_range_and_truncation(self):
        for status, headers, body in [
            (200, "Content-Length: 4", b"abcd"),
            (206, "Content-Range: bytes 11-14/100\nContent-Length: 4", b"abcd"),
            (206, "Content-Range: bytes 10-13/100\nContent-Length: 4", b"abc"),
            (206, "Content-Range: bytes 10-13/100\nContent-Encoding: gzip", b"abcd"),
        ]:
            with self.subTest(status=status, headers=headers), tempfile.TemporaryDirectory() as scratch:
                with patch.object(SAMPLE.subprocess, "run", self.response(status, headers, body)):
                    with self.assertRaises(ValueError):
                        SAMPLE.fetch_once("shard", 4, (10, 14), scratch=scratch)

    def test_oversized_request_fails_before_network(self):
        with tempfile.TemporaryDirectory() as scratch, patch.object(SAMPLE.subprocess, "run") as run:
            with self.assertRaises(ValueError):
                SAMPLE.fetch_once("shard", 4, (10, 15), scratch=scratch)
            run.assert_not_called()

    def test_metadata_length_must_match(self):
        with tempfile.TemporaryDirectory() as scratch, patch.object(
            SAMPLE.subprocess, "run", self.response(200, "Content-Length: 8", b"{}")
        ):
            with self.assertRaises(ValueError):
                SAMPLE.fetch_once("config.json", 32, scratch=scratch)


if __name__ == "__main__":
    unittest.main()
