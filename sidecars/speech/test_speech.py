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
                return iter([types.SimpleNamespace(text=" confirm ", no_speech_prob=0.0, avg_logprob=0.0)]), None

        with tempfile.TemporaryDirectory() as directory:
            recorder = Path(directory) / "pw-record"
            recorder.write_text("#!/usr/bin/env python3\nimport sys,time,signal\nsignal.signal(signal.SIGINT,lambda *_:sys.exit(0))\nsys.stdout.buffer.write(b'\\x00\\x10'*320)\nsys.stdout.buffer.flush()\ntime.sleep(60)\n")
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
                self.assertEqual(model.samples, b"\x00\x10" * 320)
                self.assertEqual(model.options, {"beam_size": 5, "language": "en", "vad_filter": True})
            finally:
                speech.close()
                os.environ["PATH"] = previous_path

    def test_hallucinated_whole_transcripts_are_empty(self):
        for text in [
            ' "THANK YOU VERY MUCH FOR WATCHING AND BYE BYE!" ',
            "thanks so much for listening", "Thank you, bye-bye.", "goodbye bye", "you",
            "please subscribe to the channel", "like and subscribe!",
            "subscribe now", "see you next time", "see you in the next video",
            "subtitles by someone", "amara captions", "transcription by someone",
            "the end",
        ]:
            with self.subTest(text=text):
                segment = types.SimpleNamespace(text=text, no_speech_prob=0, avg_logprob=0)
                self.assertEqual(module.transcript([segment]), "")

    def test_real_sentences_preserve_the_original_text(self):
        for text in ["Thank you for fixing the build.", "Say goodbye to the legacy module.",
                     "Can you please subscribe to this event?", "Put this at the end."]:
            with self.subTest(text=text):
                segment = types.SimpleNamespace(text=text, no_speech_prob=0, avg_logprob=0)
                self.assertEqual(module.transcript([segment]), text)

    def test_segments_need_both_silence_indicators_to_be_dropped(self):
        segments = [
            types.SimpleNamespace(text="discard", no_speech_prob=0.61, avg_logprob=-1.01),
            types.SimpleNamespace(text="keep probability boundary", no_speech_prob=0.6, avg_logprob=-2),
            types.SimpleNamespace(text="keep logprob boundary", no_speech_prob=0.9, avg_logprob=-1.0),
        ]
        self.assertEqual(module.transcript(iter(segments)),
                         "keep probability boundary keep logprob boundary")
        self.assertEqual(module.transcript(segments[:1]), "")

    def test_peak_reads_signed_samples(self):
        self.assertEqual(module.peak(b""), 0)
        self.assertEqual(module.peak(b"\x01\x00\xff\xff"), 1)
        self.assertEqual(module.peak(b"\x00\x80"), 32768)

    def test_silent_or_empty_capture_replies_silent_without_transcribing(self):
        class Model:
            def transcribe(self, path, **options):
                raise AssertionError("silent audio must not be transcribed")

        for samples in ["b''", "b'\\x01\\x00'*320"]:
            with self.subTest(samples=samples), tempfile.TemporaryDirectory() as directory:
                recorder = Path(directory) / "pw-record"
                recorder.write_text("#!/usr/bin/env python3\nimport sys,time,signal\nsignal.signal(signal.SIGINT,lambda *_:sys.exit(0))\nsys.stdout.buffer.write(" + samples + ")\nsys.stdout.buffer.flush()\ntime.sleep(60)\n")
                recorder.chmod(0o700)
                previous_path = os.environ["PATH"]
                os.environ["PATH"] = directory + os.pathsep + previous_path
                speech = module.Speech(Model())
                output = io.StringIO()
                try:
                    with contextlib.redirect_stdout(output):
                        speech.start(3)
                        time.sleep(0.1)
                        speech.stop(3, True)
                        deadline = time.monotonic() + 3
                        while not output.getvalue() and time.monotonic() < deadline:
                            time.sleep(0.01)
                    self.assertEqual(json.loads(output.getvalue()), {"session": 3, "error": "silent"})
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
