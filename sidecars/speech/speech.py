#!/usr/bin/env python3
"""Capture on core commands; return transcription without delivering input."""
import json
import os
import re
import select
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time
import wave

DEFAULT_LANGUAGE = "en"
MAX_HOTWORD_TOKENS = 64
MAX_LINE = 65536
MAX_AUDIO_SECONDS = 60
INTERIM_BYTES = 16000 * 2
SILENCE_PEAK = 64
DEFAULT_SPEECH_TAIL_MS = 150
MAX_SPEECH_TAIL_MS = MAX_AUDIO_SECONDS * 1000


HALLUCINATION = re.compile(
    r"(thank you|thanks)( so much| very much)?( for watching| for listening)?"
    r"( and)?( (bye|goodbye|bye bye|see you))*|(bye|goodbye|bye bye)( bye)*"
    r"|you|please subscribe.*|(like and )?subscribe.*"
    r"|see you (next time|in the next video)|subtitles by .*"
    r"|(amara|transcription by) .*|the end"
)


LOOP_MIN_REPEATS = 8
LOOP_MAX_PHRASE = 3
LOOP_MIN_SHARE = 0.6


def repetition_loop(words):
    """True when one word or short phrase repeats LOOP_MIN_REPEATS+ times in a row
    and those repeats make up most of the words."""
    words = [word for word in (re.sub(r"\W", "", w.lower()) for w in words) if word]
    for size in range(1, LOOP_MAX_PHRASE + 1):
        start = 0
        while start + size * LOOP_MIN_REPEATS <= len(words):
            unit = words[start:start + size]
            end = start + size
            while words[end:end + size] == unit:
                end += size
            repeats = (end - start) // size
            if repeats >= LOOP_MIN_REPEATS and (end - start) / len(words) >= LOOP_MIN_SHARE:
                return True
            start += 1
    return False


def transcript(segments):
    text = " ".join(
        segment.text.strip() for segment in segments
        if not (segment.no_speech_prob > 0.6 and segment.avg_logprob < -1.0)
    ).strip()
    normalized = " ".join(re.sub(r"[.!?,'\"]", "", text.lower()).replace("-", " ").split())
    if HALLUCINATION.fullmatch(normalized):
        return ""
    if repetition_loop(normalized.split()):
        log(f"dropped repetition-loop transcript ({len(normalized.split())} words): {text[:60]!r}")
        return ""
    return text


def until(segments, abort):
    """Segments, ending early once abort is set (the decode is lazy)."""
    for segment in segments:
        if abort is not None and abort.is_set():
            return
        yield segment


def peak(audio):
    samples = memoryview(audio[:len(audio) - len(audio) % 2]).cast("h")
    return max((abs(sample) for sample in samples), default=0)


def reply(value):
    print(json.dumps(value), flush=True)


def log(message, private=False, level="INFO"):
    line = f"{level} speech: {message}\n"
    try:
        with open(os.environ.get("TEHUTI_LOG_FILE", "/tmp/tehuti.log"), "a") as output:
            output.write(line)
    except OSError as error:
        if not private:
            sys.stderr.write(line)
        print(f"speech log failed: {error}", file=sys.stderr, flush=True)


def capture_sources(listing):
    names = []
    for node in listing:
        if node.get("type") != "PipeWire:Interface:Node":
            continue
        properties = node.get("info", {}).get("props", {})
        name = properties.get("node.name")
        if (properties.get("media.class") == "Audio/Source"
                and properties.get("device.class") != "monitor"
                and isinstance(name, str) and name and not name.endswith(".monitor")):
            if name not in names:
                names.append(name)
    return names


def default_source(listing):
    """The node name PipeWire's metadata names as the default capture source, if it says."""
    for node in listing:
        if node.get("type") != "PipeWire:Interface:Metadata":
            continue
        for entry in node.get("metadata", []):
            if entry.get("key") == "default.audio.source":
                value = entry.get("value")
                name = value.get("name") if isinstance(value, dict) else value
                if isinstance(name, str) and name:
                    return name
    return None


def chosen_source():
    """The microphone picked in Tehuti's settings: "all", "auto", a node name, or None."""
    root = os.environ.get("XDG_CONFIG_HOME") or os.path.join(os.path.expanduser("~"), ".config")
    try:
        with open(os.path.join(root, "tehuti", "speech-source"), encoding="utf-8") as choice:
            return choice.readline().strip() or None
    except OSError:
        return None


def disabled_sources():
    """Microphone node names turned off in Tehuti's settings; never recorded, also under "all"."""
    root = os.environ.get("XDG_CONFIG_HOME") or os.path.join(os.path.expanduser("~"), ".config")
    try:
        with open(os.path.join(root, "tehuti", "microphones-disabled.json"), encoding="utf-8") as saved:
            names = json.load(saved)
    except (OSError, ValueError):
        return set()
    return {name for name in names if isinstance(name, str)} if isinstance(names, list) else set()


def list_sources(missing=None):
    """The pinned source, else the settings choice, else every capture device, else the default.

    `missing` is called with the saved source name when it is not among the capture devices."""
    pinned = os.environ.get("TEHUTI_SPEECH_SOURCE")
    if pinned:
        return [pinned]
    choice = chosen_source()
    off = disabled_sources()
    if choice == "auto" and not off:
        return [None]
    listed = True
    default = None
    try:
        listing = subprocess.run(
            ["pw-dump"],
            capture_output=True, check=True, timeout=2,
        ).stdout
        dump = json.loads(listing)
        names = capture_sources(dump)
        default = default_source(dump)
    except (OSError, subprocess.SubprocessError, ValueError, KeyError, TypeError, AttributeError) as error:
        print(f"speech source listing failed: {error}", file=sys.stderr, flush=True)
        names = []
        listed = False
    if choice == "auto":
        # The default source cannot be recorded by name here: record it only when it is known to be on.
        return [None] if default is not None and default not in off else []
    enabled = [name for name in names if name not in off]
    if choice in enabled:
        return [choice]
    if missing is not None and choice not in (None, "all", "auto") and choice not in off and listed:
        missing(choice)
    if names:
        return enabled
    # Discovery found nothing: the default source stands in only when no microphone is off.
    return [] if off else [None]


REMOTE_FINAL_SECONDS = 30
REMOTE_INTERIM_SECONDS = 3
MAX_REMOTE_REPLY = 65536


def remote_sockets():
    """Language to absolute remote socket path, from `[remote]` in speech.toml.

    Entries with a bad language code or a path that is not absolute after expansion are
    skipped with a warning."""
    root = os.environ.get("XDG_CONFIG_HOME") or os.path.join(os.path.expanduser("~"), ".config")
    try:
        import tomllib
        with open(os.path.join(root, "tehuti", "speech.toml"), "rb") as saved:
            table = tomllib.load(saved).get("remote", {})
    except (OSError, ValueError):
        return {}
    if not isinstance(table, dict):
        return {}
    sockets = {}
    for language, path in table.items():
        expanded = os.path.expandvars(path) if isinstance(path, str) else ""
        if not re.fullmatch(r"[a-z]{2,8}", language):
            log(f"speech.toml: remote language {language!r} skipped", level="WARN")
        elif not os.path.isabs(expanded) or "$" in expanded:
            log(f"speech.toml: remote path for {language} is not absolute; skipped", level="WARN")
        else:
            sockets[language] = expanded
    return sockets


def remote_transcribe(path, audio, prompt, final, abort=None):
    """Text from a remote transcription socket within one total deadline.

    Raises on error, busy, a bad or oversized reply, or the deadline."""
    deadline = time.monotonic() + (REMOTE_FINAL_SECONDS if final else REMOTE_INTERIM_SECONDS)

    def remaining():
        left = deadline - time.monotonic()
        if left <= 0 or (abort is not None and abort.is_set()):
            raise TimeoutError("remote transcription deadline")
        return left

    request = (b"G" if final else b"J") + prompt.encode("utf-8") + b"\0" + audio
    with socket.socket(socket.AF_UNIX) as connection:
        connection.settimeout(remaining())
        connection.connect(path)
        connection.setblocking(False)
        sent = 0
        while sent < len(request):
            _, writable, _ = select.select([], [connection], [], min(remaining(), 0.25))
            if writable:
                try:
                    sent += connection.send(request[sent:sent + 65536])
                except BlockingIOError:
                    pass
        connection.shutdown(socket.SHUT_WR)
        data = bytearray()
        while True:
            readable, _, _ = select.select([connection], [], [], min(remaining(), 0.25))
            if not readable:
                continue
            chunk = connection.recv(4096)
            if not chunk:
                break
            data.extend(chunk)
            if len(data) > MAX_REMOTE_REPLY:
                raise ValueError("remote reply too large")
    answer = json.loads(bytes(data))
    if not isinstance(answer, dict):
        raise ValueError("remote reply is not an object")
    if answer.get("busy"):
        raise RuntimeError("busy")
    if isinstance(answer.get("text"), str):
        return answer["text"]
    raise RuntimeError(str(answer.get("error", "bad remote reply")))


def speech_tails(value):
    tails = {"default": DEFAULT_SPEECH_TAIL_MS}
    if not value:
        return tails
    for entry in value.split(","):
        name, separator, milliseconds = entry.strip().partition("=")
        name, milliseconds = name.strip(), milliseconds.strip()
        if (not separator or not name or not milliseconds.isascii()
                or not milliseconds.isdecimal() or len(milliseconds) > 5
                or int(milliseconds) > MAX_SPEECH_TAIL_MS):
            print("WARNING speech: invalid TEHUTI_SPEECH_TAIL_MS entry; using fallback",
                  file=sys.stderr, flush=True)
            tails.pop(name, None)
            continue
        tails[name] = int(milliseconds)
    tails.setdefault("default", DEFAULT_SPEECH_TAIL_MS)
    return tails


class Recording:
    def __init__(self, source, process):
        self.source = source
        self.process = process
        self.audio = bytearray()
        self.peak = 0
        self.scanned = 0
        self.reader = None

    def extend(self, chunk):
        self.audio.extend(chunk)
        whole = len(self.audio) - len(self.audio) % 2
        self.peak = max(self.peak, peak(self.audio[self.scanned:whole]))
        self.scanned = whole


class Take:
    """One push-to-talk, recorded from every source at once."""
    def __init__(self, session, prompt="", language=DEFAULT_LANGUAGE, remote=None):
        self.remote = remote  # the backend, resolved once: a socket path, else the local model
        self.session = session
        self.prompt = prompt
        self.language = language
        self.recordings = []
        self.cancelled = threading.Event()
        self.interim_abort = threading.Event()
        self.worker = None
        self.stopped = time.monotonic()
        self.backend = "local"
        self.waited = 0.0
        self.decoded = 0.0

    def poll(self):
        if any(r.process.poll() is None for r in self.recordings):
            return None
        return 0


def choose(recordings):
    """The recording with the strongest signal; ties keep listing order."""
    return max(recordings, key=lambda r: r.peak)


class Speech:
    def __init__(self, model, sources=None, remotes=None):
        self.model = model
        self.remotes = remote_sockets if remotes is None else remotes
        self.hotwords = ""
        self.warned_missing = False
        self.sources = sources or (lambda: list_sources(self.warn_missing_source))
        self.tails = speech_tails(os.environ.get("TEHUTI_SPEECH_TAIL_MS"))
        self.releasing = None
        self.capture = None
        self.session = None
        self.model_lock = threading.Lock()
        self.transcriptions = threading.BoundedSemaphore(2)
        self.interims = threading.BoundedSemaphore(1)

    def set_vocabulary(self, vocabulary):
        words, seen = [], set()
        for word in vocabulary:
            word = " ".join(word.split())
            if not word or word.casefold() in seen:
                continue
            seen.add(word.casefold())
            candidate = ", ".join([*words, word])
            if len(self.model.hf_tokenizer.encode(" " + candidate, add_special_tokens=False).ids) <= MAX_HOTWORD_TOKENS:
                words.append(word)
        self.hotwords = ", ".join(words)

    def warn_missing_source(self, name):
        if self.warned_missing:
            return
        self.warned_missing = True
        log(f"saved source {name} is missing; capturing from the other sources", level="WARN")
        reply({"warning": "source missing"})

    def cancel_tail(self):
        if self.releasing is not None:
            self.releasing.cancelled.set()
            self.releasing.worker.join()
            self.releasing = None

    def start(self, session, prompt="", language=DEFAULT_LANGUAGE):
        self.cancel_tail()
        if self.capture is not None:
            return
        take = Take(session, prompt, language, self.remotes().get(language))
        for source in self.sources():
            target = [] if source is None else ["--target", source]
            try:
                process = subprocess.Popen(
                    ["pw-record", *target, "--raw", "--format", "s16", "--rate", "16000",
                     "--channels", "1", "-"],
                    stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                )
            except OSError:
                print("speech capture source failed to start", file=sys.stderr, flush=True)
                continue
            take.recordings.append(Recording(source or "default", process))
        if not take.recordings:
            reply({"session": session, "error": "no capture source"})
            return
        self.capture = take
        self.session = session
        for recording in take.recordings:
            recording.reader = threading.Thread(
                target=self.drain, args=(session, take, recording), daemon=True,
            )
            recording.reader.start()

    def drain(self, session, take, recording):
        capture = recording.process
        remaining = MAX_AUDIO_SECONDS * 16000 * 2
        next_interim = INTERIM_BYTES
        while remaining:
            chunk = capture.stdout.read1(min(4096, remaining))
            if not chunk:
                break
            remaining -= len(chunk)
            recording.extend(chunk)
            if len(recording.audio) >= next_interim:
                if choose(take.recordings) is recording:
                    self.interim(session, bytes(recording.audio), take)
                next_interim = len(recording.audio) + INTERIM_BYTES
        if remaining == 0 and capture.poll() is None:
            capture.send_signal(signal.SIGINT)

    def transcribe_audio(self, audio, prompt="", *, retry=True, abort=None, beam_size=5,
                         language=DEFAULT_LANGUAGE):
        priming = {"hotwords": self.hotwords} if self.hotwords else {}
        if prompt:
            priming["initial_prompt"] = prompt
        with tempfile.NamedTemporaryFile(suffix=".wav") as recording:
            with wave.open(recording.name, "wb") as output:
                output.setnchannels(1)
                output.setsampwidth(2)
                output.setframerate(16000)
                output.writeframes(audio)
            segments, _ = self.model.transcribe(
                recording.name, beam_size=beam_size, language=language, vad_filter=True, **priming,
                vad_parameters={"threshold": 0.3, "min_speech_duration_ms": 80,
                                "speech_pad_ms": 300},
            )
            text = transcript(until(segments, abort))
            if text or (abort is not None and abort.is_set()):
                return text
            log(f"VAD returned empty text for clip ({len(audio)} bytes)")
            if not retry or len(audio) < 16000 * 2 * 1.5 or peak(audio) <= SILENCE_PEAK * 4:
                return ""
            log("retrying clip without VAD")
            segments, _ = self.model.transcribe(
                recording.name, beam_size=5, language=language, vad_filter=False,
                temperature=0, condition_on_previous_text=False, **priming,
            )
            return transcript(segments)

    def transcribe_take(self, take, audio, *, final):
        """Transcribe with the backend the take resolved at its start.

        Only a local final takes the model lock, and an interim never does: the lock only
        orders finals, so a final cannot wait behind an interim's decode. A remote failure
        falls back to the local model for a final; an interim then yields nothing."""
        if take.remote:
            try:
                began = time.monotonic()
                text = remote_transcribe(take.remote, audio, take.prompt, final,
                                         None if final else take.interim_abort)
                if final:
                    take.backend, take.decoded = "remote", time.monotonic() - began
                return text
            except (OSError, ValueError, RuntimeError, TimeoutError) as error:
                log(f"remote transcription failed ({error}); "
                    + ("using the local model" if final else "interim skipped"), level="WARN")
                if not final:
                    return ""
        if final:
            take.backend = "local"
            asked = time.monotonic()
            with self.model_lock:
                began = time.monotonic()
                try:
                    return self.transcribe_audio(audio, take.prompt, language=take.language)
                finally:
                    take.waited, take.decoded = began - asked, time.monotonic() - began
        return self.transcribe_audio(audio, take.prompt, retry=False,
                                     abort=take.interim_abort, beam_size=1,
                                     language=take.language)

    def interim(self, session, audio, capture):
        if not self.interims.acquire(blocking=False):
            return

        def finish():
            try:
                if self.capture is not capture or peak(audio) <= SILENCE_PEAK:
                    return
                text = self.transcribe_take(capture, audio, final=False)
                if self.capture is capture and not capture.interim_abort.is_set() and text:
                    reply({"session": session, "interim": text})
            except Exception as error:
                print(f"interim transcription failed: {error}", file=sys.stderr)
            finally:
                self.interims.release()

        threading.Thread(target=finish, daemon=True).start()

    def stop(self, session, transcribe):
        if (not transcribe and self.releasing is not None
                and session == self.releasing.session):
            self.cancel_tail()
        if self.capture is None or session != self.session:
            return
        take = self.capture
        self.capture = self.session = None
        take.interim_abort.set()
        take.stopped = time.monotonic()
        if transcribe:
            self.cancel_tail()
            self.releasing = take
            take.worker = threading.Thread(
                target=self.finish_take, args=(session, take, True), daemon=True,
            )
            take.worker.start()
        else:
            self.finish_take(session, take, False)

    def finish_take(self, session, take, transcribe):
        released = time.monotonic()
        recordings = sorted(take.recordings,
                            key=lambda r: self.tails.get(r.source, self.tails["default"]))
        for recording in recordings:
            if recording.process.poll() is not None:
                continue
            tail = self.tails.get(recording.source, self.tails["default"]) / 1000
            if transcribe:
                take.cancelled.wait(max(0, released + tail - time.monotonic()))
            if recording.process.poll() is None:
                recording.process.send_signal(signal.SIGINT)
        for recording in take.recordings:
            try:
                recording.process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                recording.process.kill()
                recording.process.wait()
        finished = []
        for recording in take.recordings:
            recording.reader.join(timeout=3)
            recording.process.stdout.close()
            if not recording.reader.is_alive():
                finished.append(recording)
        if not transcribe or take.cancelled.is_set():
            return

        def answer(value):
            reply({"session": session, **value})
            log(f"session {session}: language {take.language}, backend {take.backend}, "
                f"lock wait {take.waited:.2f}s, decode {take.decoded:.2f}s, "
                f"stop to reply {time.monotonic() - take.stopped:.2f}s")

        if not finished:
            answer({"error": "capture did not finish"})
            return
        if not self.transcriptions.acquire(blocking=False):
            answer({"error": "busy"})
            return

        def finish():
            try:
                winner = choose(finished)
                log(f"session {session}: source {winner.source}, "
                    f"{len(winner.audio)} bytes, peak {winner.peak}", private=True)
                if winner.peak <= SILENCE_PEAK:
                    answer({"error": "silent"})
                    return
                text = self.transcribe_take(take, bytes(winner.audio), final=True)
                answer({"text": text})
            except Exception as error:
                print(f"speech transcription failed: {error}", file=sys.stderr)
                answer({"error": str(error)})
            finally:
                self.transcriptions.release()

        threading.Thread(target=finish, daemon=True).start()

    def close(self):
        self.cancel_tail()
        if self.capture is not None:
            self.stop(self.session, False)


def run(stream, speech, device=None):
    ready = {"ready": True}
    if device is not None:
        ready["device"], ready["compute_type"] = device
    reply(ready)
    try:
        while True:
            line = stream.readline(MAX_LINE + 1)
            if not line:
                break
            if len(line) > MAX_LINE or not line.endswith("\n"):
                raise ValueError("speech command exceeds line limit")
            command = json.loads(line)
            if (isinstance(command, dict) and command
                    and not {"start", "stop", "transcribe", "vocabulary"}.intersection(command)):
                log("unknown speech command ignored")
                continue
            if (isinstance(command, dict) and "start" in command
                    and set(command) <= {"start", "prompt", "language"}
                    and type(command["start"]) is int and command["start"] >= 0
                    and isinstance(command.get("prompt", ""), str) and "\0" not in command.get("prompt", "")
                    and isinstance(command.get("language", DEFAULT_LANGUAGE), str)
                    and re.fullmatch(r"[a-z]{2,8}", command.get("language", DEFAULT_LANGUAGE))):
                speech.start(command["start"], " ".join(command.get("prompt", "").split()[:100]),
                             command.get("language", DEFAULT_LANGUAGE))
            elif (set(command) == {"stop", "transcribe"}
                  and type(command["stop"]) is int and command["stop"] >= 0
                  and type(command["transcribe"]) is bool):
                speech.stop(command["stop"], command["transcribe"])
            elif (set(command) == {"vocabulary"} and isinstance(command["vocabulary"], list)
                  and all(isinstance(word, str) and "\0" not in word
                          for word in command["vocabulary"])):
                speech.set_vocabulary(command["vocabulary"])
            else:
                raise ValueError("invalid speech command")
    finally:
        speech.close()


if __name__ == "__main__":
    sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
    import device
    device.reexec_with_cuda_libraries()
    choice = device.select(os.environ.get("TEHUTI_SPEECH_MODEL", "small"), log)
    speech = Speech(choice.model)
    def terminate(_signal, _frame):
        raise SystemExit(0)
    signal.signal(signal.SIGTERM, terminate)
    run(sys.stdin, speech, (choice.device, choice.compute_type))
