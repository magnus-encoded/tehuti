import contextlib
import importlib.util
import io
import json
import os
import socket
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
    def test_a_pinned_verb_redecodes_the_same_audio_in_its_locale(self):
        class Model:
            def __init__(self):
                self.calls = []

            def transcribe(self, path, **options):
                with wave.open(path) as recording:
                    audio = recording.readframes(recording.getnframes())
                self.calls.append((audio, options))
                text = "Invoce, snakk med say" if options["language"] == "no" else "Invoke, chat with say"
                return iter([types.SimpleNamespace(text=text, no_speech_prob=0, avg_logprob=0)]), None

        model = Model()
        speech = module.Speech(model, remotes=dict)
        take = module.Take(1, "", "no")
        take.verbs = [{"word": "invoke", "locale": "en", "hints": ["chat", "channel"]}]
        audio = b"\x00\x10" * 100
        self.assertEqual(speech.transcribe_take(take, audio, final=True), "invoke chat with say")
        self.assertEqual([call[0] for call in model.calls], [audio, audio])
        self.assertEqual([call[1]["language"] for call in model.calls], ["no", "en"])
        self.assertIn("chat", model.calls[1][1]["initial_prompt"])
        self.assertIn("channel", model.calls[1][1]["hotwords"])

    def test_sound_matching_obeys_first_word_priority_and_word_boundaries(self):
        verbs = ["ada", {"word": "invoke", "locale": "en"}, {"word": "literal", "locale": "en"}]
        self.assertEqual(module.match_verb("Invoce, usage report", verbs), (verbs[1], "usage report"))
        self.assertEqual(module.match_verb("Literel says hi", verbs), (verbs[2], "says hi"))
        for text in ("invoke", "invoke   ", "please invoke chat", "Invoked functions return",
                     "Literally nothing changed", "Adam was here"):
            self.assertIsNone(module.match_verb(text, verbs), text)
        self.assertEqual(module.match_verb("Ada hello", ["ada", {"word": "ada", "locale": "en"}]),
                         ("ada", "hello"))

    def test_only_a_matching_pinned_final_gets_one_extra_decode(self):
        class Model:
            def __init__(self, texts):
                self.texts = iter(texts)
                self.languages = []

            def transcribe(self, path, **options):
                self.languages.append(options["language"])
                text = next(self.texts)
                return iter([types.SimpleNamespace(text=text, no_speech_prob=0, avg_logprob=0)]), None

        pinned = {"word": "invoke", "locale": "en", "hints": ["chat"]}
        cases = [
            (["Invoce snakk", "Something else"], [pinned], True, "Invoce snakk", ["no", "en"]),
            (["Invoce snakk", ""], [pinned], True, "Invoce snakk", ["no", "en"]),
            (["Invoce snakk"], ["invoke"], True, "Invoce snakk", ["no"]),
            (["please invoke chat"], [pinned], True, "please invoke chat", ["no"]),
            (["Invoce snakk"], [pinned], False, "Invoce snakk", ["no"]),
            (["Invoce snakk"], ["invoke", pinned], True, "Invoce snakk", ["no"]),
        ]
        for texts, verbs, final, expected, languages in cases:
            with self.subTest(texts=texts, verbs=verbs, final=final):
                model = Model(texts)
                speech = module.Speech(model, remotes=dict)
                take = module.Take(1, "", "no", verbs=verbs)
                self.assertEqual(speech.transcribe_take(take, b"\x00\x10" * 100, final=final), expected)
                self.assertEqual(model.languages, languages)

    def test_start_snapshots_validated_verb_definitions(self):
        class Capture:
            def __init__(self):
                self.verbs = None

            def start(self, session, prompt, language, verbs=()):
                self.verbs = verbs

            def close(self):
                pass

        verbs = ["ada", {"word": "invoke", "locale": "en", "hints": ["chat"]}]
        capture = Capture()
        with contextlib.redirect_stdout(io.StringIO()):
            module.run(io.StringIO(json.dumps({"start": 1, "language": "no", "verbs": verbs}) + "\n"), capture)
        self.assertEqual(capture.verbs, verbs)
        for invalid in ([{"word": "invoke", "locale": "xx"}],
                        [{"word": "invoke", "locale": "en-US"}],
                        [{"word": "invoke", "locale": "en", "hints": ["x"] * 17}],
                        [7], [{"word": "two words", "locale": "en"}]):
            with self.assertRaises(ValueError), contextlib.redirect_stdout(io.StringIO()):
                module.run(io.StringIO(json.dumps({"start": 2, "verbs": invalid}) + "\n"), capture)

    def test_start_prompt_is_optional_bounded_and_used_on_both_vad_attempts(self):
        class Capture:
            def __init__(self):
                self.calls = []

            def start(self, session, prompt="", language="en"):
                self.calls.append((session, prompt))

            def close(self):
                pass

        capture = Capture()
        commands = [{"start": 1}, {"start": 2, "prompt": "Tehuti " * 101}]
        with contextlib.redirect_stdout(io.StringIO()):
            module.run(io.StringIO("".join(json.dumps(c) + "\n" for c in commands)), capture)
        self.assertEqual(capture.calls, [(1, ""), (2, " ".join(["Tehuti"] * 100))])

        class Model:
            def __init__(self):
                self.calls = []

            def transcribe(self, path, **options):
                self.calls.append(options)
                return iter([]), None

        model = Model()
        speech = module.Speech(model)
        with tempfile.TemporaryDirectory() as directory, patch.dict(
                os.environ, {"TEHUTI_LOG_FILE": directory + "/log"}):
            speech.transcribe_audio(b"\x00\x10" * 24000, "chat with yell")
        self.assertEqual([call["initial_prompt"] for call in model.calls],
                         ["chat with yell", "chat with yell"])

    def test_start_language_is_passed_validated_and_defaults_to_english(self):
        class Capture:
            def __init__(self):
                self.calls = []

            def start(self, session, prompt="", language="en"):
                self.calls.append((session, language))

            def close(self):
                pass

        capture = Capture()
        commands = [{"start": 1}, {"start": 2, "language": "no"},
                    {"start": 3, "prompt": "x", "language": "no"}]
        with contextlib.redirect_stdout(io.StringIO()):
            module.run(io.StringIO("".join(json.dumps(c) + "\n" for c in commands)), capture)
        self.assertEqual(capture.calls, [(1, "en"), (2, "no"), (3, "no")])
        for bad in ({"start": 4, "language": "NO"}, {"start": 4, "language": 7},
                    {"start": 4, "language": "../x"}):
            with self.assertRaises(ValueError), contextlib.redirect_stdout(io.StringIO()):
                module.run(io.StringIO(json.dumps(bad) + "\n"), capture)

    def test_the_session_language_reaches_the_model_for_interims_and_the_final(self):
        class Model:
            def __init__(self):
                self.languages = []

            def transcribe(self, path, **options):
                self.languages.append(options["language"])
                return iter([]), None

        model = Model()
        speech = module.Speech(model, remotes=dict)
        with tempfile.TemporaryDirectory() as directory, patch.dict(
                os.environ, {"TEHUTI_LOG_FILE": directory + "/log"}):
            speech.transcribe_audio(b"\x00\x10" * 24000, language="no", retry=False, beam_size=1)
            speech.transcribe_audio(b"\x00\x10" * 24000, language="no", retry=False)
            speech.transcribe_audio(b"\x00\x10" * 24000, retry=False)
        self.assertEqual(model.languages, ["no", "no", "en"])

    def serve(self, directory, reply, count=1, received=None, trickle=False):
        path = os.path.join(directory, "remote.sock")
        listener = socket.socket(socket.AF_UNIX)
        listener.bind(path)
        listener.listen(4)
        self.addCleanup(listener.close)

        def one():
            connection, _ = listener.accept()
            with connection:
                data = bytearray()
                while True:
                    chunk = connection.recv(65536)
                    if not chunk:
                        break
                    data.extend(chunk)
                if received is not None:
                    received.append(bytes(data))
                if trickle:
                    try:
                        for _ in range(100):
                            connection.sendall(b" ")
                            time.sleep(0.05)
                    except OSError:
                        pass
                    return
                connection.sendall(reply)

        def run_all():
            for _ in range(count):
                one()

        threading.Thread(target=run_all, daemon=True).start()
        return path

    def take(self, speech, language="no"):
        return module.Take(1, "chat", language, speech.remotes().get(language))

    def test_a_remote_language_goes_to_the_socket_and_leaves_the_model_idle(self):
        received = []

        class Model:
            def transcribe(self, *args, **options):
                raise AssertionError("the local model must stay idle")

        with tempfile.TemporaryDirectory() as directory:
            path = self.serve(directory, b'{"text": "hei"}\n', 2, received)
            speech = module.Speech(Model(), remotes=lambda: {"no": path})
            audio = b"\x00\x10" * 100
            take = self.take(speech)
            self.assertEqual(speech.transcribe_take(take, audio, final=True), "hei")
            self.assertEqual(speech.transcribe_take(take, audio, final=False), "hei")
        self.assertEqual(received, [b"Gchat\0" + audio, b"Jchat\0" + audio])

    def test_a_trickling_server_hits_the_total_deadline(self):
        with tempfile.TemporaryDirectory() as directory:
            path = self.serve(directory, b"", 1, trickle=True)
            with patch.object(module, "REMOTE_INTERIM_SECONDS", 0.5):
                started = time.monotonic()
                with self.assertRaises(TimeoutError):
                    module.remote_transcribe(path, b"\x00\x10" * 10, "", final=False)
                self.assertLess(time.monotonic() - started, 3)

    def test_a_dead_socket_falls_back_to_the_local_final_with_the_takes_language(self):
        class Model:
            def __init__(self):
                self.languages = []

            def transcribe(self, path, **options):
                self.languages.append(options["language"])
                return iter([]), None

        model = Model()
        with tempfile.TemporaryDirectory() as directory, patch.dict(
                os.environ, {"TEHUTI_LOG_FILE": directory + "/log"}):
            speech = module.Speech(model, remotes=lambda: {"no": directory + "/dead.sock"})
            take = self.take(speech)
            self.assertEqual(speech.transcribe_take(take, b"\x00\x10" * 24000, final=True), "")
            self.assertTrue(model.languages and set(model.languages) == {"no"})
            seen = len(model.languages)
            self.assertEqual(speech.transcribe_take(take, b"\x00\x10" * 24000, final=False), "")
            self.assertEqual(len(model.languages), seen, "an interim does not fall back")

    def test_a_busy_reply_falls_back_for_the_final(self):
        class Model:
            calls = 0

            def transcribe(self, path, **options):
                Model.calls += 1
                return iter([]), None

        with tempfile.TemporaryDirectory() as directory, patch.dict(
                os.environ, {"TEHUTI_LOG_FILE": directory + "/log"}):
            path = self.serve(directory, b'{"busy": true}\n')
            speech = module.Speech(Model(), remotes=lambda: {"no": path})
            speech.transcribe_take(self.take(speech), b"\x00\x10" * 24000, final=True)
        self.assertGreaterEqual(Model.calls, 1)

    def test_an_oversized_reply_is_refused(self):
        with tempfile.TemporaryDirectory() as directory:
            path = self.serve(directory, b" " * (module.MAX_REMOTE_REPLY + 5000))
            with self.assertRaises(ValueError):
                module.remote_transcribe(path, b"\x00\x10" * 10, "", final=True)

    def test_speech_toml_maps_languages_to_absolute_sockets_and_skips_bad_entries(self):
        with tempfile.TemporaryDirectory() as directory:
            os.mkdir(os.path.join(directory, "tehuti"))
            with open(os.path.join(directory, "tehuti", "speech.toml"), "w") as saved:
                saved.write('[remote]\nno = "$XDG_RUNTIME_DIR/t.sock"\nbad = 3\nNo = "/a"\n'
                            'rel = "relative.sock"\nunset = "$NOPE_UNSET/x.sock"\n')
            with patch.dict(os.environ, {"XDG_CONFIG_HOME": directory, "XDG_RUNTIME_DIR": "/run/x",
                                         "TEHUTI_LOG_FILE": directory + "/log"}):
                self.assertEqual(module.remote_sockets(), {"no": "/run/x/t.sock"})
            with patch.dict(os.environ, {"XDG_CONFIG_HOME": directory + "/none"}):
                self.assertEqual(module.remote_sockets(), {})

    def test_ready_reply_names_the_chosen_device_and_compute_type(self):
        class Speech:
            def close(self):
                pass

        with contextlib.redirect_stdout(io.StringIO()) as output:
            module.run(io.StringIO(""), Speech(), ("cuda", "int8"))
        self.assertEqual(json.loads(output.getvalue()),
                         {"ready": True, "device": "cuda", "compute_type": "int8"})

    def test_transport_logs_unknown_messages_and_continues_capture(self):
        class Speech:
            def __init__(self):
                self.sessions = []
                self.closed = False

            def start(self, session, prompt="", language="en"):
                self.sessions.append((session, None))

            def stop(self, session, transcribe):
                self.sessions[-1] = (session, transcribe)

            def close(self):
                self.closed = True

        speech = Speech()
        commands = ('{"future_command":true}\n{"start":7}\n'
                    '{"another_command":{"option":1}}\n'
                    '{"stop":7,"transcribe":true}\n')
        with tempfile.TemporaryDirectory() as directory:
            log_path = Path(directory) / "speech.log"
            with patch.dict(os.environ, {"TEHUTI_LOG_FILE": str(log_path)}), \
                    contextlib.redirect_stdout(io.StringIO()) as output:
                module.run(io.StringIO(commands), speech)
            self.assertEqual(log_path.read_text().splitlines(), [
                "INFO speech: unknown speech command ignored",
                "INFO speech: unknown speech command ignored",
            ])
        self.assertEqual(speech.sessions, [(7, True)])
        self.assertTrue(speech.closed)
        self.assertEqual(json.loads(output.getvalue()), {"ready": True})

    def test_vocabulary_primes_both_vad_attempts_with_a_short_hotword_list(self):
        class Model:
            hf_tokenizer = types.SimpleNamespace(encode=lambda text, **_: types.SimpleNamespace(ids=list(text.encode("utf-8"))))

            def __init__(self):
                self.calls = []

            def transcribe(self, path, **options):
                self.calls.append(options)
                return iter([]), None

        model = Model()
        speech = module.Speech(model)
        with tempfile.TemporaryDirectory() as directory, patch.dict(
                os.environ, {"TEHUTI_LOG_FILE": directory + "/log"}):
            speech.set_vocabulary(["Orchid", "Orchid", "Open browser", "x" * 500, "y" * 48, "Atlas"])
            speech.transcribe_audio(b"\x00\x10" * 160)
            for options in model.calls:
                self.assertEqual(options["hotwords"], "Orchid, Open browser, Atlas")
                self.assertNotIn("initial_prompt", options)
            model.calls.clear()
            speech.set_vocabulary([])
            speech.transcribe_audio(b"\x00\x10" * 160)
            self.assertNotIn("hotwords", model.calls[0])

    def test_transport_refreshes_vocabulary_and_rejects_invalid_words(self):
        class Speech:
            def __init__(self):
                self.words = None

            def set_vocabulary(self, words):
                self.words = words

            def close(self):
                pass

        speech = Speech()
        with contextlib.redirect_stdout(io.StringIO()):
            module.run(io.StringIO('{"vocabulary":["Orchid"]}\n{"vocabulary":[]}\n'), speech)
            self.assertEqual(speech.words, [])
            for value in [None, "word", [1], ["bad\0word"]]:
                with self.subTest(value=value), self.assertRaises(ValueError):
                    module.run(io.StringIO(json.dumps({"vocabulary": value}) + "\n"), speech)

    def test_tail_map_validates_entries_and_warns_with_fallback(self):
        for invalid in ["-1", "1.5", "nan", "inf", "60001", "9" * 5000, ""]:
            with self.subTest(invalid=invalid), contextlib.redirect_stderr(io.StringIO()) as warning:
                self.assertEqual(module.speech_tails("remote=" + invalid + ",default=75"),
                                 {"default": 75})
                self.assertIn("WARNING", warning.getvalue())
        with contextlib.redirect_stderr(io.StringIO()) as warning:
            self.assertEqual(module.speech_tails("broken,default=-1,=12"), {"default": 150})
            self.assertEqual(warning.getvalue().count("WARNING"), 3)
        self.assertEqual(module.speech_tails("remote=1200,default=150,instant=0"),
                         {"remote": 1200, "default": 150, "instant": 0})
        self.assertEqual(module.speech_tails(None), {"default": 150})

    def test_sources_stop_at_their_own_tails_and_late_remote_audio_can_win(self):
        self.run_tail_capture("finish")

    def test_press_during_source_tail_discards_old_capture(self):
        self.run_tail_capture("restart")

    def test_cancellation_during_source_tail_discards_capture(self):
        self.run_tail_capture("cancel")

    def run_tail_capture(self, action):
        class Model:
            def transcribe(self, path, **options):
                with wave.open(path) as clip:
                    self.samples = clip.readframes(clip.getnframes())
                return iter([types.SimpleNamespace(text="late words", no_speech_prob=0,
                                                   avg_logprob=0)]), None

        with tempfile.TemporaryDirectory() as directory:
            recorder = Path(directory) / "pw-record"
            recorder.write_text("""#!/usr/bin/env python3
import sys,time,signal,pathlib
signal.signal(signal.SIGINT,lambda *_:sys.exit(0))
source=sys.argv[2]
pathlib.Path(source+'.ready').touch()
if source == 'broken': sys.exit(1)
sys.stdout.buffer.write(b'\\x00\\x01'*10)
sys.stdout.buffer.flush()
while not pathlib.Path('released').exists(): time.sleep(0.005)
time.sleep(0.7)
sys.stdout.buffer.write(b'\\x00\\x30'*10)
sys.stdout.buffer.flush()
time.sleep(60)
""")
            recorder.chmod(0o700)
            model = Model()
            output = io.StringIO()
            with patch.dict(os.environ, {"PATH": directory + os.pathsep + os.environ["PATH"],
                                        "TEHUTI_SPEECH_TAIL_MS": "remote=1200,broken=60000,default=150",
                                        "TEHUTI_LOG_FILE": directory + "/log"}), \
                    contextlib.chdir(directory), contextlib.redirect_stdout(output):
                speech = module.Speech(model, sources=lambda: ["local", "remote", "broken"])
                try:
                    speech.start(42)
                    take = speech.capture
                    deadline = time.monotonic() + 3
                    while not all(Path(name + ".ready").exists() for name in ["local", "remote", "broken"]):
                        self.assertLess(time.monotonic(), deadline)
                        time.sleep(0.005)
                    speech.stop(42, True)
                    Path("released").touch()
                    time.sleep(0.4)
                    self.assertIsNotNone(take.recordings[0].process.poll())
                    self.assertIsNone(take.recordings[1].process.poll())
                    self.assertEqual(output.getvalue(), "")
                    if action != "finish":
                        if action == "restart":
                            speech.start(43)
                        else:
                            speech.stop(42, False)
                        self.assertIsNotNone(take.recordings[1].process.poll())
                        self.assertEqual(output.getvalue(), "")
                        if action == "restart":
                            self.assertIsNone(speech.capture.poll())
                    else:
                        time.sleep(0.5)
                        self.assertIsNone(take.recordings[1].process.poll())
                        while not output.getvalue() and time.monotonic() < deadline:
                            time.sleep(0.01)
                        self.assertEqual(json.loads(output.getvalue()),
                                         {"session": 42, "text": "late words"})
                        self.assertEqual(model.samples, b"\x00\x01" * 10 + b"\x00\x30" * 10)
                finally:
                    speech.close()

    def test_empty_interim_never_retries_even_with_long_loud_audio(self):
        class Model:
            def __init__(self):
                self.calls = []

            def transcribe(self, path, **options):
                self.calls.append(options)
                text = "" if options["vad_filter"] else "unwanted retry"
                return iter([types.SimpleNamespace(
                    text=text, no_speech_prob=0, avg_logprob=0,
                )]), None

        model = Model()
        speech = module.Speech(model)
        take = module.Take(7)
        speech.capture = take
        with tempfile.TemporaryDirectory() as directory, patch.dict(
                os.environ, {"TEHUTI_LOG_FILE": directory + "/log"}), \
                contextlib.redirect_stdout(io.StringIO()) as output:
            speech.interim(7, b"\x00\x10" * 32000, take)
            self.assertTrue(speech.interims.acquire(timeout=3))
            speech.interims.release()
        self.assertEqual(output.getvalue(), "")
        self.assertEqual(len(model.calls), 1)

    def test_quiet_or_short_final_clip_skips_retry(self):
        class Model:
            def __init__(self):
                self.calls = []

            def transcribe(self, path, **options):
                self.calls.append(options)
                return iter([]), None

        for sample, frames in [(b"\x00\x01", 24000),
                               (b"\x00\xff", 24000),
                               (b"\x00\x10", 23999)]:
            with self.subTest(sample=sample, frames=frames), \
                    tempfile.TemporaryDirectory() as directory, patch.dict(
                        os.environ, {"TEHUTI_LOG_FILE": directory + "/log"}):
                model = Model()
                self.assertEqual(module.Speech(model).transcribe_audio(sample * frames), "")
                self.assertEqual(len(model.calls), 1)

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
                        text = module.Speech(model).transcribe_audio(b"\x00\x10" * 24000)
                self.assertEqual(text, retry_text)
                self.assertEqual(model.calls, [
                    {"beam_size": 5, "language": "en", "vad_filter": True,
                     "vad_parameters": {"threshold": 0.3, "min_speech_duration_ms": 80,
                                        "speech_pad_ms": 300}},
                    {"beam_size": 5, "language": "en", "vad_filter": False,
                     "temperature": 0, "condition_on_previous_text": False},
                ])
                self.assertEqual(log_path.read_text().splitlines(), [
                    "INFO speech: VAD returned empty text for clip (48000 bytes)",
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
        capture = module.Take(0, "Tehuti")
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

    def test_final_is_not_queued_behind_a_running_interim(self):
        interim_started = threading.Event()
        segment = types.SimpleNamespace(text="w", no_speech_prob=0, avg_logprob=0)

        class Model:
            def transcribe(self, path, **options):
                if options.get("vad_filter") and not interim_started.is_set():
                    interim_started.set()

                    def slow():
                        for _ in range(100):
                            time.sleep(0.05)
                            yield segment
                    return slow(), None
                return iter([segment]), None

        class Process:
            stdout = io.BytesIO()

            def poll(self):
                return 0

            def wait(self, timeout=None):
                return 0

        speech = module.Speech(Model(), sources=lambda: [None])
        take = module.Take(4)
        recording = module.Recording("default", Process())
        recording.extend(b"\x00\x10" * 32000)
        recording.reader = threading.Thread(target=lambda: None)
        recording.reader.start()
        take.recordings.append(recording)
        speech.capture, speech.session = take, 4
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            speech.interim(4, bytes(recording.audio), take)
            self.assertTrue(interim_started.wait(3))
            began = time.monotonic()
            speech.stop(4, True)
            speech.releasing.worker.join(3)
            while not output.getvalue() and time.monotonic() - began < 3:
                time.sleep(0.01)
            elapsed = time.monotonic() - began
        self.assertLess(elapsed, 2.0, "interim would hold the model for 5 s")
        self.assertEqual([json.loads(l) for l in output.getvalue().splitlines()],
                         [{"session": 4, "text": "w"}])

    def final_take(self, speech, session=4, reader_alive=False):
        class Process:
            stdout = io.BytesIO()

            def poll(self):
                return 0

            def wait(self, timeout=None):
                return 0

        take = module.Take(session)
        recording = module.Recording("default", Process())
        recording.extend(b"\x00\x10" * 32000)
        gate = threading.Event()
        recording.reader = threading.Thread(target=lambda: gate.wait(10), daemon=True)
        recording.reader.start()
        if not reader_alive:
            gate.set()
            recording.reader.join()
        take.recordings.append(recording)
        speech.capture, speech.session = take, session
        return take, gate

    def run_stop(self, speech, session=4):
        output = io.StringIO()
        with contextlib.redirect_stdout(output), tempfile.TemporaryDirectory() as directory, \
                patch.dict(os.environ, {"TEHUTI_LOG_FILE": directory + "/log"}):
            speech.stop(session, True)
            speech.releasing.worker.join(1)
            deadline = time.monotonic() + 5
            while not output.getvalue() and time.monotonic() < deadline:
                time.sleep(0.01)
            log = Path(directory, "log").read_text() if Path(directory, "log").exists() else ""
        return [json.loads(l) for l in output.getvalue().splitlines()], log

    def test_final_behind_slow_blocking_interim_replies_within_one_transcription(self):
        started = threading.Event()
        segment = types.SimpleNamespace(text="w", no_speech_prob=0, avg_logprob=0)

        class Model:
            def transcribe(self, path, **options):
                if options.get("beam_size") == 1 and not started.is_set():
                    started.set()
                    time.sleep(0.5)
                return iter([segment]), None

        speech = module.Speech(Model(), sources=lambda: [None])
        take, _ = self.final_take(speech)
        with contextlib.redirect_stdout(io.StringIO()):
            speech.interim(4, bytes(take.recordings[0].audio), take)
        self.assertTrue(started.wait(3))
        began = time.monotonic()
        replies, log = self.run_stop(speech)
        self.assertEqual(replies, [{"session": 4, "text": "w"}], "discarded interim publishes nothing")
        self.assertLess(time.monotonic() - began, 1.5)
        self.assertIn("stop to reply", log)

    class OneWorkerModel:
        """Like CTranslate2 with one worker: calls run one at a time, in arrival order."""
        def __init__(self, interim_seconds=0.0):
            self.worker = threading.Lock()
            self.interim_seconds = interim_seconds
            self.started = threading.Event()
            self.calls = []

        def transcribe(self, path, **options):
            with self.worker:
                with wave.open(path) as clip:
                    seconds = clip.getnframes() / 16000
                self.calls.append((options.get("beam_size"), seconds))
                if options.get("beam_size") == 1:
                    self.started.set()
                    time.sleep(self.interim_seconds)
                segment = types.SimpleNamespace(text="w", no_speech_prob=0, avg_logprob=0)
                return iter([segment]), None

    def test_a_final_arriving_mid_interim_waits_for_that_one_decode_only(self):
        model = self.OneWorkerModel(interim_seconds=0.6)
        speech = module.Speech(model, sources=lambda: [None])
        take, _ = self.final_take(speech)
        with contextlib.redirect_stdout(io.StringIO()):
            speech.interim(4, bytes(take.recordings[0].audio), take)
        self.assertTrue(model.started.wait(3))
        began = time.monotonic()
        replies, log = self.run_stop(speech)
        self.assertEqual(replies, [{"session": 4, "text": "w"}])
        self.assertGreater(time.monotonic() - began, 0.3, "the final queued behind the decode")
        self.assertLess(time.monotonic() - began, 1.5)
        self.assertRegex(log, r"session 4: language en, backend local, audio 2\.0s, "
                              r"queue wait 0\.\d\ds, decode 0\.\d\ds, retry no, "
                              r"stop to reply \d+\.\d\ds")

    def test_no_interim_starts_while_a_final_is_pending(self):
        model = self.OneWorkerModel()
        speech = module.Speech(model, sources=lambda: [None])
        take, _ = self.final_take(speech)
        with speech.schedule:
            speech.finals_pending = 1
        with contextlib.redirect_stdout(io.StringIO()):
            speech.interim(4, bytes(take.recordings[0].audio), take)
            time.sleep(0.3)
        self.assertEqual(model.calls, [])

    def test_an_interim_decodes_only_the_last_five_seconds(self):
        model = self.OneWorkerModel()
        speech = module.Speech(model, sources=lambda: [None])
        take = module.Take(4)
        speech.capture = take
        with contextlib.redirect_stdout(io.StringIO()):
            speech.interim(4, b"\x00\x10" * 16000 * 12, take)
            deadline = time.monotonic() + 3
            while not model.calls and time.monotonic() < deadline:
                time.sleep(0.01)
        self.assertEqual(model.calls, [(1, 5.0)])

    def test_silent_and_busy_sessions_log_no_backend(self):
        speech = module.Speech(types.SimpleNamespace(), sources=lambda: [None])
        take, _ = self.final_take(speech)
        take.recordings[0].peak = 0
        replies, log = self.run_stop(speech)
        self.assertEqual(replies, [{"session": 4, "error": "silent"}])
        self.assertIn("no decode", log)
        self.assertNotIn("backend", log)

    def test_a_remote_interim_leaves_the_local_final_unqueued(self):
        speech = module.Speech(self.OneWorkerModel(), remotes=dict)
        with tempfile.TemporaryDirectory() as directory:
            path = self.serve(directory, b"", 1, trickle=True)
            speech.remotes = lambda: {"no": path}
            take = self.take(speech)
            result = []
            worker = threading.Thread(target=lambda: result.append(
                speech.transcribe_take(take, b"\x00\x10" * 100, final=False)), daemon=True)
            worker.start()
            time.sleep(0.2)
            self.assertFalse(speech.decoding, "a remote interim never holds the model")
            local = self.take(speech, "en")
            local.remote = None
            began = time.monotonic()
            self.assertEqual(speech.transcribe_take(local, b"\x00\x10" * 100, final=True), "w")
            self.assertLess(time.monotonic() - began, 0.5)
            take.interim_abort.set()
            worker.join(timeout=3)
            self.assertEqual(result, [""])

    def test_stops_behind_a_slow_final_queue_and_each_gets_its_own_reply(self):
        release = threading.Event()
        started = threading.Event()
        segment = types.SimpleNamespace(text="w", no_speech_prob=0, avg_logprob=0)
        calls = []

        class Model:
            def transcribe(self, path, **options):
                calls.append(path)
                if len(calls) == 1:
                    started.set()
                    release.wait(10)
                return iter([segment]), None

        speech = module.Speech(Model(), sources=lambda: [None])
        output = io.StringIO()
        try:
            with contextlib.redirect_stdout(output), \
                    tempfile.TemporaryDirectory() as directory, \
                    patch.dict(os.environ, {"TEHUTI_LOG_FILE": directory + "/log"}):
                for session in (1, 2, 3):
                    self.final_take(speech, session)
                    speech.stop(session, True)
                    speech.releasing.worker.join(1)
                    if session == 1:
                        self.assertTrue(started.wait(3))
                release.set()
                deadline = time.monotonic() + 5
                while len(output.getvalue().splitlines()) < 3 and time.monotonic() < deadline:
                    time.sleep(0.01)
            replies = sorted(json.loads(l)["session"] for l in output.getvalue().splitlines()
                             if '"text": "w"' in l)
            self.assertEqual(replies, [1, 2, 3], "no stop fails fast with busy")
        finally:
            release.set()

    def test_busy_transcriptions_reply_with_an_error(self):
        speech = module.Speech(types.SimpleNamespace(), sources=lambda: [None])
        self.final_take(speech)
        for _ in range(module.MAX_PENDING_FINALS):
            self.assertTrue(speech.transcriptions.acquire(blocking=False))
        replies, _ = self.run_stop(speech)
        self.assertEqual(replies, [{"session": 4, "error": "busy"}])

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

    def test_repetition_loop_of_primed_verbs_is_dropped_and_logged(self):
        loop = "build. " + "Invoke. " * 70
        with tempfile.TemporaryDirectory() as directory, patch.dict(
                os.environ, {"TEHUTI_LOG_FILE": directory + "/log"}):
            segment = types.SimpleNamespace(text=loop, no_speech_prob=0, avg_logprob=0)
            self.assertEqual(module.transcript([segment]), "")
            self.assertIn("repetition-loop", Path(directory, "log").read_text())

    def test_phrase_and_punctuation_variant_loops_are_dropped_and_logged(self):
        for text in ["go down " * 12, "open the file " * 8, "Invoke; invoke\u2026 INVOKE, " * 8]:
            with self.subTest(text=text), tempfile.TemporaryDirectory() as directory, patch.dict(
                    os.environ, {"TEHUTI_LOG_FILE": directory + "/log"}):
                segment = types.SimpleNamespace(text=text, no_speech_prob=0, avg_logprob=0)
                self.assertEqual(module.transcript([segment]), "")
                self.assertIn("repetition-loop", Path(directory, "log").read_text())

    def test_repeat_boundary_seven_passes_and_eight_drops(self):
        with tempfile.TemporaryDirectory() as directory, patch.dict(
                os.environ, {"TEHUTI_LOG_FILE": directory + "/log"}):
            for count, expected in [(7, "down " * 7), (8, "")]:
                segment = types.SimpleNamespace(text="down " * count, no_speech_prob=0, avg_logprob=0)
                self.assertEqual(module.transcript([segment]), expected.strip())

    def test_deliberate_repeats_and_diluted_repeats_go_through(self):
        for text in ["down down down", "down down down down",
                     "scroll down down down down then open the file and save it now"]:
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

    def test_settings_choice_narrows_discovery_unless_its_source_is_gone(self):
        listing = json.dumps([{"type": "PipeWire:Interface:Node",
                               "info": {"props": {"node.name": name,
                                                  "media.class": "Audio/Source"}}}
                              for name in ["front", "headset"]]).encode()
        dump = types.SimpleNamespace(stdout=listing)
        cases = [(None, ["front", "headset"]), ("all", ["front", "headset"]),
                 ("auto", [None]), ("headset", ["headset"]), ("unplugged", ["front", "headset"])]
        with tempfile.TemporaryDirectory() as directory, \
                patch.dict(os.environ, {"XDG_CONFIG_HOME": directory}), \
                patch.object(module.subprocess, "run", return_value=dump):
            os.environ.pop("TEHUTI_SPEECH_SOURCE", None)
            (Path(directory) / "tehuti").mkdir()
            for choice, expected in cases:
                with self.subTest(choice=choice):
                    path = Path(directory) / "tehuti" / "speech-source"
                    if choice is None:
                        path.unlink(missing_ok=True)
                    else:
                        path.write_text(choice + "\n")
                    self.assertEqual(module.list_sources(), expected)

    def test_turned_off_microphones_are_never_listed_even_under_all(self):
        listing = json.dumps([{"type": "PipeWire:Interface:Node",
                               "info": {"props": {"node.name": name, "media.class": "Audio/Source"}}}
                              for name in ["front", "headset"]]).encode()
        dump = types.SimpleNamespace(stdout=listing)
        cases = [("all", ["front"]), ("front", ["front"]), ("headset", ["front"]),
                 ("all", []), ("all", ["front", "headset"])]
        with tempfile.TemporaryDirectory() as directory, \
                patch.dict(os.environ, {"XDG_CONFIG_HOME": directory}), \
                patch.object(module.subprocess, "run", return_value=dump):
            os.environ.pop("TEHUTI_SPEECH_SOURCE", None)
            (Path(directory) / "tehuti").mkdir()
            for choice, off in cases:
                with self.subTest(choice=choice, off=off):
                    (Path(directory) / "tehuti" / "speech-source").write_text(choice + "\n")
                    (Path(directory) / "tehuti" / "microphones-disabled.json").write_text(json.dumps(off))
                    expected = [name for name in ["front", "headset"] if name not in off]
                    self.assertEqual(module.list_sources(), expected)

    def test_auto_honours_the_off_list_and_the_fallback_does_too(self):
        def dump(default):
            nodes = [{"type": "PipeWire:Interface:Node",
                      "info": {"props": {"node.name": name, "media.class": "Audio/Source"}}}
                     for name in ["front", "headset"]]
            meta = {"type": "PipeWire:Interface:Metadata",
                    "metadata": [{"key": "default.audio.source", "value": {"name": default}}]}
            return types.SimpleNamespace(stdout=json.dumps(nodes + [meta]).encode())
        cases = [("auto", [], "front", [None]), ("auto", ["headset"], "front", [None]),
                 ("auto", ["front"], "front", []), ("all", ["front", "headset"], "front", [])]
        for choice, off, default, expected in cases:
            with self.subTest(choice=choice, off=off), tempfile.TemporaryDirectory() as directory, \
                    patch.dict(os.environ, {"XDG_CONFIG_HOME": directory}), \
                    patch.object(module.subprocess, "run", return_value=dump(default)):
                os.environ.pop("TEHUTI_SPEECH_SOURCE", None)
                (Path(directory) / "tehuti").mkdir()
                (Path(directory) / "tehuti" / "speech-source").write_text(choice + "\n")
                (Path(directory) / "tehuti" / "microphones-disabled.json").write_text(json.dumps(off))
                self.assertEqual(module.list_sources(), expected)
        with tempfile.TemporaryDirectory() as directory, \
                patch.dict(os.environ, {"XDG_CONFIG_HOME": directory}), \
                patch.object(module.subprocess, "run", side_effect=OSError("no pw-dump")):
            os.environ.pop("TEHUTI_SPEECH_SOURCE", None)
            (Path(directory) / "tehuti").mkdir()
            self.assertEqual(module.list_sources(), [None])
            (Path(directory) / "tehuti" / "microphones-disabled.json").write_text(json.dumps(["front"]))
            self.assertEqual(module.list_sources(), [])

    def test_a_missing_saved_source_is_reported_and_falls_back(self):
        listing = json.dumps([{"type": "PipeWire:Interface:Node",
                               "info": {"props": {"node.name": "front", "media.class": "Audio/Source"}}}]).encode()
        dump = types.SimpleNamespace(stdout=listing)
        for choice, missing in [("unplugged", ["unplugged"]), ("front", []), ("all", []), ("auto", []), (None, [])]:
            with self.subTest(choice=choice), tempfile.TemporaryDirectory() as directory, \
                    patch.dict(os.environ, {"XDG_CONFIG_HOME": directory}), \
                    patch.object(module.subprocess, "run", return_value=dump):
                os.environ.pop("TEHUTI_SPEECH_SOURCE", None)
                (Path(directory) / "tehuti").mkdir()
                if choice is not None:
                    (Path(directory) / "tehuti" / "speech-source").write_text(choice + "\n")
                seen = []
                self.assertEqual(module.list_sources(seen.append), ["front"] if choice != "auto" else [None])
                self.assertEqual(seen, missing)

    def test_speech_warns_in_log_and_to_the_compositor_when_the_saved_source_is_missing(self):
        speech = module.Speech(object())
        output = io.StringIO()
        with tempfile.TemporaryDirectory() as directory, patch.dict(
                os.environ, {"TEHUTI_LOG_FILE": directory + "/log"}), contextlib.redirect_stdout(output):
            speech.warn_missing_source("unplugged")
            speech.warn_missing_source("unplugged")
            logged = Path(directory, "log").read_text()
        self.assertEqual(json.loads(output.getvalue()), {"warning": "source missing"})
        self.assertIn("WARN speech: saved source unplugged is missing", logged)
        self.assertEqual(logged.count("WARN"), 1)

    def test_all_sources_record_at_once_and_failed_sources_are_skipped(self):
        class Model:
            def transcribe(self, path, **options):
                with wave.open(path) as clip:
                    self.samples = clip.readframes(clip.getnframes())
                self.calls = getattr(self, "calls", []) + [(self.samples, options["vad_filter"])]
                text = "hi"
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
                self.assertEqual(model.calls, [(b"\x00\x30" * 320, True)])
                self.assertEqual(log_path.read_text().splitlines()[0],
                                 "INFO speech: session 5: source front, 640 bytes, peak 12288")
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
