import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import tempfile
import time
import threading
import types
import unittest
from unittest.mock import patch
import wave

spec = importlib.util.spec_from_file_location("speech", Path(__file__).with_name("speech.py"))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class SpeechTests(unittest.TestCase):
    def test_empty_vad_transcript_retries_once_without_vad(self):
        for retry_text in ["confirm", ""]:
            with self.subTest(retry_text=retry_text), tempfile.TemporaryDirectory() as directory:
                class Model:
                    def __init__(self):
                        self.calls = []

                    def transcribe(self, path, **options):
                        self.calls.append(options)
                        text = "" if len(self.calls) == 1 else retry_text
                        return iter([types.SimpleNamespace(
                            text=text, no_speech_prob=0, avg_logprob=0,
                        )]), None

                model = Model()
                log_path = Path(directory) / "speech.log"
                with patch.dict(os.environ, {"TEHUTI_LOG_FILE": str(log_path)}):
                    with contextlib.redirect_stderr(io.StringIO()):
                        text = module.Speech(model).transcribe_audio(b"\x00\x10" * 16000)
                self.assertEqual(text, retry_text)
                self.assertEqual(model.calls, [
                    {"beam_size": 5, "language": "en", "vad_filter": True,
                     "vad_parameters": {"threshold": 0.3, "min_speech_duration_ms": 80,
                                        "speech_pad_ms": 300}},
                    {"beam_size": 5, "language": "en", "vad_filter": False},
                ])
                self.assertEqual(log_path.read_text().splitlines(), [
                    "INFO speech: VAD returned empty text for clip (32000 bytes)",
                    "INFO speech: retrying clip without VAD",
                ])

    def test_nonempty_vad_transcript_does_not_retry(self):
        class Model:
            def __init__(self):
                self.calls = []

            def transcribe(self, path, **options):
                self.calls.append(options)
                return iter([types.SimpleNamespace(
                    text="confirm", no_speech_prob=0, avg_logprob=0,
                )]), None

        model = Model()
        with tempfile.TemporaryDirectory() as directory:
            log_path = Path(directory) / "speech.log"
            with patch.dict(os.environ, {"TEHUTI_LOG_FILE": str(log_path)}):
                self.assertEqual(module.Speech(model).transcribe_audio(b"\x00\x10" * 16000),
                                 "confirm")
            self.assertFalse(log_path.exists())
        self.assertEqual(len(model.calls), 1)
        self.assertTrue(model.calls[0]["vad_filter"])

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
            speech = module.Speech(model, sources=lambda: [None])
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
                self.assertEqual(model.options, {
                    "beam_size": 5, "language": "en", "vad_filter": True,
                    "vad_parameters": {"threshold": 0.3, "min_speech_duration_ms": 80,
                                       "speech_pad_ms": 300},
                })
            finally:
                speech.close()
                os.environ["PATH"] = previous_path

    def test_live_preview_emits_text_while_record_is_held(self):
        class Model:
            def transcribe(self, path, **options):
                return iter([types.SimpleNamespace(text="Live Words", no_speech_prob=0, avg_logprob=0)]), None

        with tempfile.TemporaryDirectory() as directory:
            recorder = Path(directory) / "pw-record"
            recorder.write_text("#!/usr/bin/env python3\nimport sys,time,signal\nsignal.signal(signal.SIGINT,lambda *_:sys.exit(0))\nsys.stdout.buffer.write(b'\\x00\\x10'*16000)\nsys.stdout.buffer.flush()\ntime.sleep(60)\n")
            recorder.chmod(0o700)
            previous_path = os.environ["PATH"]
            os.environ["PATH"] = directory + os.pathsep + previous_path
            speech = module.Speech(Model(), sources=lambda: [None])
            output = io.StringIO()
            try:
                with contextlib.redirect_stdout(output):
                    speech.start(9)
                    deadline = time.monotonic() + 3
                    while not output.getvalue() and time.monotonic() < deadline:
                        time.sleep(0.01)
                    self.assertEqual(json.loads(output.getvalue()), {"session": 9, "interim": "Live Words"})
                    self.assertIsNone(speech.capture.poll(), "preview leaves capture running")
                    speech.stop(9, True)
                    while len(output.getvalue().splitlines()) < 2 and time.monotonic() < deadline:
                        time.sleep(0.01)
                self.assertEqual(json.loads(output.getvalue().splitlines()[-1]), {"session": 9, "text": "Live Words"})
            finally:
                speech.close()
                os.environ["PATH"] = previous_path

    def test_cancelled_inflight_preview_is_dropped_and_preview_work_is_bounded(self):
        started, finish = threading.Event(), threading.Event()

        class Model:
            def transcribe(self, path, **options):
                started.set()
                finish.wait(3)
                return iter([types.SimpleNamespace(text="old", no_speech_prob=0, avg_logprob=0)]), None

        speech = module.Speech(Model(), sources=lambda: [None])
        capture = object()
        speech.capture = capture
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            speech.interim(0, b"\x00\x10" * 16000, capture)
            self.assertTrue(started.wait(3))
            self.assertFalse(speech.interims.acquire(blocking=False), "at most one preview transcribes")
            speech.capture = None
            finish.set()
            self.assertTrue(speech.interims.acquire(timeout=3), "preview releases its budget")
            speech.interims.release()
        self.assertEqual(output.getvalue(), "", "cancelled capture cannot publish a preview")

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
                speech = module.Speech(Model(), sources=lambda: [None])
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

    def recording(self, source, *chunks):
        recording = module.Recording(source, None)
        for chunk in chunks:
            recording.extend(chunk)
        return recording

    def test_loudest_source_wins_over_a_live_but_silent_default(self):
        dead = self.recording("quiet", b"\x01\x00" * 800)
        jack = self.recording("front", b"\x00\x00" * 400 + b"\x00\x20" + b"\x00\x00" * 400)
        self.assertIs(module.choose([dead, jack]), jack)
        self.assertEqual((dead.peak, jack.peak), (1, 0x2000))

    def test_ties_keep_the_first_listed_source(self):
        first, second = self.recording("a", b"\x00\x01"), self.recording("b", b"\x00\x01")
        self.assertIs(module.choose([first, second]), first)

    def test_peak_survives_samples_split_across_chunks(self):
        self.assertEqual(self.recording("a", b"\x00", b"\x40", b"\x00\x00").peak, 0x4000)

    def test_source_listing_keeps_capture_devices_and_drops_monitors(self):
        def node(name, media_class="Audio/Source", **properties):
            return {"type": "PipeWire:Interface:Node",
                    "info": {"props": {"node.name": name, "media.class": media_class,
                                       **properties}}}
        listing = [
            node("output.monitor"), node("microphone-a"),
            node("renamed", **{"device.class": "monitor"}),
            node("output", "Audio/Sink"), node("microphone-b"),
            node("microphone-a"), {"type": "PipeWire:Interface:Device"},
        ]
        self.assertEqual(module.capture_sources(listing), ["microphone-a", "microphone-b"])

    def test_pinned_source_overrides_discovery(self):
        previous = os.environ.get("TEHUTI_SPEECH_SOURCE")
        os.environ["TEHUTI_SPEECH_SOURCE"] = "front"
        try:
            self.assertEqual(module.list_sources(), ["front"])
        finally:
            if previous is None:
                del os.environ["TEHUTI_SPEECH_SOURCE"]
            else:
                os.environ["TEHUTI_SPEECH_SOURCE"] = previous

    def test_all_sources_record_at_once_and_failed_sources_are_skipped(self):
        class Model:
            def transcribe(self, path, **options):
                with wave.open(path) as clip:
                    self.samples = clip.readframes(clip.getnframes())
                self.calls = getattr(self, "calls", []) + [(self.samples, options["vad_filter"])]
                text = "" if options["vad_filter"] else "hi"
                return iter([types.SimpleNamespace(text=text, no_speech_prob=0, avg_logprob=0)]), None

        with tempfile.TemporaryDirectory() as directory:
            recorder = Path(directory) / "pw-record"
            recorder.write_text("#!/usr/bin/env python3\nimport sys,time,signal\nsignal.signal(signal.SIGINT,lambda *_:sys.exit(0))\ntarget=sys.argv[2]\nif target=='broken': sys.exit(1)\nsys.stdout.buffer.write((b'\\x00\\x30' if target=='front' else b'\\x01\\x00')*320)\nsys.stdout.buffer.flush()\ntime.sleep(60)\n")
            recorder.chmod(0o700)
            previous_path = os.environ["PATH"]
            os.environ["PATH"] = directory + os.pathsep + previous_path
            model = Model()
            speech = module.Speech(model, sources=lambda: ["quiet", "broken", "front"])
            output, log = io.StringIO(), io.StringIO()
            log_path = Path(directory) / "speech.log"
            try:
                with patch.dict(os.environ, {"TEHUTI_LOG_FILE": str(log_path)}), contextlib.redirect_stdout(output), contextlib.redirect_stderr(log):
                    speech.start(5)
                    time.sleep(0.2)
                    speech.stop(5, True)
                    deadline = time.monotonic() + 3
                    while not output.getvalue() and time.monotonic() < deadline:
                        time.sleep(0.01)
                self.assertEqual(json.loads(output.getvalue()), {"session": 5, "text": "hi"})
                self.assertEqual(model.samples, b"\x00\x30" * 320)
                self.assertEqual(model.calls, [(b"\x00\x30" * 320, True),
                                               (b"\x00\x30" * 320, False)])
                self.assertEqual(log_path.read_text().splitlines(), [
                    "INFO speech: session 5: source front, 640 bytes, peak 12288",
                    "INFO speech: VAD returned empty text for clip (640 bytes)",
                    "INFO speech: retrying clip without VAD",
                ])
                self.assertNotIn("front", log.getvalue())
                self.assertNotIn("quiet", log.getvalue())
            finally:
                speech.close()
                os.environ["PATH"] = previous_path

    def test_private_source_log_never_falls_back_to_stderr(self):
        with tempfile.TemporaryDirectory() as directory:
            with patch.dict(os.environ, {"TEHUTI_LOG_FILE": directory}):
                stderr = io.StringIO()
                with contextlib.redirect_stderr(stderr):
                    module.log("source microphone-a", private=True)
                self.assertNotIn("microphone-a", stderr.getvalue())

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
