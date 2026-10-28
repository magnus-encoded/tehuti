"""Typed speech client command-line behaviour."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

CLIENT = Path(__file__).resolve().parent / 'tehuti-say'


class SayTests(unittest.TestCase):
    def test_phrase_reaches_the_runtime_fifo(self):
        with tempfile.TemporaryDirectory() as directory:
            fifo = Path(directory) / 'tehuti-say'
            os.mkfifo(fifo, 0o600)
            reader = os.open(fifo, os.O_RDWR | os.O_NONBLOCK)
            self.addCleanup(os.close, reader)
            result = subprocess.run([str(CLIENT), 'hello', 'world'],
                                    env={**os.environ, 'XDG_RUNTIME_DIR': directory},
                                    capture_output=True, timeout=3)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(os.read(reader, 4096), b'hello world\n')

    def test_unavailable_fifo_fails_without_waiting(self):
        with tempfile.TemporaryDirectory() as directory:
            result = subprocess.run([str(CLIENT), 'hello'],
                                    env={**os.environ, 'XDG_RUNTIME_DIR': directory},
                                    capture_output=True, timeout=3)
            self.assertNotEqual(result.returncode, 0)

    def test_disconnected_reader_and_non_fifo_paths_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            fifo = Path(directory) / 'tehuti-say'
            env = {**os.environ, 'XDG_RUNTIME_DIR': directory}
            os.mkfifo(fifo, 0o600)
            result = subprocess.run([str(CLIENT), 'hello'], env=env,
                                    capture_output=True, timeout=3)
            self.assertNotEqual(result.returncode, 0)
            fifo.unlink()
            fifo.write_text('keep')
            result = subprocess.run([str(CLIENT), 'hello'], env=env,
                                    capture_output=True, timeout=3)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(fifo.read_text(), 'keep')

    def test_multiline_and_oversized_phrases_do_not_write(self):
        with tempfile.TemporaryDirectory() as directory:
            fifo = Path(directory) / 'tehuti-say'
            os.mkfifo(fifo, 0o600)
            reader = os.open(fifo, os.O_RDWR | os.O_NONBLOCK)
            self.addCleanup(os.close, reader)
            for phrase in ('hello\nworld', 'x' * 65537):
                result = subprocess.run([str(CLIENT), phrase],
                                        env={**os.environ, 'XDG_RUNTIME_DIR': directory},
                                        capture_output=True, timeout=3)
                self.assertNotEqual(result.returncode, 0)
            with self.assertRaises(BlockingIOError):
                os.read(reader, 4096)
