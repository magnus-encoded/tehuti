import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import tempfile
import time
import types
import unittest
import wave

spec = importlib.util.spec_from_file_location("speech", Path(__file__).with_name("speech.py"))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class SpeechTests(unittest.TestCase):
    def test_recording_finishes_on_release_and_transcribes_complete_audio(self):
        class Model:
            def transcribe(self, path, **options):
                with wave.open(path) as clip:
                    self.format = (clip.getframerate(), clip.getnchannels(), clip.getsampwidth())
                    self.samples = clip.readframes(clip.getnframes())
                self.options = options
                return iter([types.SimpleNamespace(text=" confirm ")]), None

        with tempfile.TemporaryDirectory() as directory:
            recorder = Path(directory) / "pw-record"
            recorder.write_text("#!/usr/bin/env python3\nimport sys,time,signal\nsignal.signal(signal.SIGINT,lambda *_:sys.exit(0))\nsys.stdout.buffer.write(b'\\x01\\x00'*320)\nsys.stdout.buffer.flush()\ntime.sleep(60)\n")
            recorder.chmod(0o700)
            previous_path = os.environ["PATH"]
            os.environ["PATH"] = directory + os.pathsep + previous_path
            model = Model()
            speech = module.Speech(model)
            output = io.StringIO()
            try:
                with contextlib.redirect_stdout(output):
                    speech.start(7)
                    time.sleep(0.1)
                    self.assertIsNone(speech.capture.poll(), "capture remains running while held")
                    speech.stop(7, True)
                    deadline = time.monotonic() + 3
                    while not output.getvalue() and time.monotonic() < deadline:
                        time.sleep(0.01)
                self.assertEqual(json.loads(output.getvalue()), {"session": 7, "text": "confirm"})
                self.assertEqual(model.format, (16000, 1, 2))
                self.assertEqual(model.samples, b"\x01\x00" * 320)
                self.assertEqual(model.options, {"beam_size": 5, "language": "en", "vad_filter": True})
            finally:
                speech.close()
                os.environ["PATH"] = previous_path

    def test_invalid_command_cancels_capture_on_transport_exit(self):
        class Capture:
            def __init__(self):
                self.closed = False
            def close(self):
                self.closed = True
        capture = Capture()
        with contextlib.redirect_stdout(io.StringIO()):
            with self.assertRaises(ValueError):
                module.run(io.StringIO('{"stop":0,"transcribe":true,"unexpected":1}\n'), capture)
        self.assertTrue(capture.closed)


if __name__ == "__main__":
    unittest.main()
