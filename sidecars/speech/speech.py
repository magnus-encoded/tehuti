#!/usr/bin/env python3
"""Capture on core commands; return transcription without delivering input."""
import json
import os
import re
import signal
import subprocess
import sys
import tempfile
import threading
import wave

MAX_LINE = 65536
MAX_AUDIO_SECONDS = 60
INTERIM_BYTES = 16000 * 2
SILENCE_PEAK = 64


HALLUCINATION = re.compile(
    r"(thank you|thanks)( so much| very much)?( for watching| for listening)?"
    r"( and)?( (bye|goodbye|bye bye|see you))*|(bye|goodbye|bye bye)( bye)*"
    r"|you|please subscribe.*|(like and )?subscribe.*"
    r"|see you (next time|in the next video)|subtitles by .*"
    r"|(amara|transcription by) .*|the end"
)


def transcript(segments):
    text = " ".join(
        segment.text.strip() for segment in segments
        if not (segment.no_speech_prob > 0.6 and segment.avg_logprob < -1.0)
    ).strip()
    normalized = " ".join(re.sub(r"[.!?,'\"]", "", text.lower()).replace("-", " ").split())
    return "" if HALLUCINATION.fullmatch(normalized) else text


def peak(audio):
    samples = memoryview(audio[:len(audio) - len(audio) % 2]).cast("h")
    return max((abs(sample) for sample in samples), default=0)


def reply(value):
    print(json.dumps(value), flush=True)


def log(message, private=False):
    line = f"INFO speech: {message}\n"
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


def list_sources():
    """The pinned source, else every capture device, else the default source."""
    pinned = os.environ.get("TEHUTI_SPEECH_SOURCE")
    if pinned:
        return [pinned]
    try:
        listing = subprocess.run(
            ["pw-dump"],
            capture_output=True, check=True, timeout=2,
        ).stdout
        names = capture_sources(json.loads(listing))
    except (OSError, subprocess.SubprocessError, ValueError, KeyError, TypeError, AttributeError) as error:
        print(f"speech source listing failed: {error}", file=sys.stderr, flush=True)
        names = []
    return names or [None]


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
    def __init__(self):
        self.recordings = []

    def poll(self):
        if any(r.process.poll() is None for r in self.recordings):
            return None
        return 0


def choose(recordings):
    """The recording with the strongest signal; ties keep listing order."""
    return max(recordings, key=lambda r: r.peak)


class Speech:
    def __init__(self, model, sources=list_sources):
        self.model = model
        self.sources = sources
        self.capture = None
        self.session = None
        self.model_lock = threading.Lock()
        self.transcriptions = threading.BoundedSemaphore(2)
        self.interims = threading.BoundedSemaphore(1)

    def start(self, session):
        if self.capture is not None:
            return
        take = Take()
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

    def transcribe_audio(self, audio):
        with tempfile.NamedTemporaryFile(suffix=".wav") as recording:
            with wave.open(recording.name, "wb") as output:
                output.setnchannels(1)
                output.setsampwidth(2)
                output.setframerate(16000)
                output.writeframes(audio)
            segments, _ = self.model.transcribe(
                recording.name, beam_size=5, language="en", vad_filter=True,
                vad_parameters={"threshold": 0.3, "min_speech_duration_ms": 80,
                                "speech_pad_ms": 300},
            )
            text = transcript(segments)
            if text:
                return text
            log(f"VAD returned empty text for clip ({len(audio)} bytes)")
            log("retrying clip without VAD")
            segments, _ = self.model.transcribe(
                recording.name, beam_size=5, language="en", vad_filter=False,
            )
            return transcript(segments)

    def interim(self, session, audio, capture):
        if not self.interims.acquire(blocking=False):
            return

        def finish():
            try:
                if self.capture is not capture or peak(audio) <= SILENCE_PEAK:
                    return
                if not self.model_lock.acquire(blocking=False):
                    return
                try:
                    text = self.transcribe_audio(audio)
                finally:
                    self.model_lock.release()
                if self.capture is capture and text:
                    reply({"session": session, "interim": text})
            except Exception as error:
                print(f"interim transcription failed: {error}", file=sys.stderr)
            finally:
                self.interims.release()

        threading.Thread(target=finish, daemon=True).start()

    def stop(self, session, transcribe):
        if self.capture is None or session != self.session:
            return
        take = self.capture
        self.capture = self.session = None
        for recording in take.recordings:
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
        if not transcribe or not finished:
            return
        if not self.transcriptions.acquire(blocking=False):
            reply({"session": session, "text": ""})
            return

        def finish():
            try:
                winner = choose(finished)
                log(f"session {session}: source {winner.source}, "
                    f"{len(winner.audio)} bytes, peak {winner.peak}", private=True)
                if winner.peak <= SILENCE_PEAK:
                    reply({"session": session, "error": "silent"})
                    return
                with self.model_lock:
                    text = self.transcribe_audio(bytes(winner.audio))
                reply({"session": session, "text": text})
            except Exception as error:
                print(f"speech transcription failed: {error}", file=sys.stderr)
                reply({"session": session, "error": str(error)})
            finally:
                self.transcriptions.release()

        threading.Thread(target=finish, daemon=True).start()

    def close(self):
        if self.capture is not None:
            self.stop(self.session, False)


def run(stream, speech):
    reply({"ready": True})
    try:
        while True:
            line = stream.readline(MAX_LINE + 1)
            if not line:
                break
            if len(line) > MAX_LINE or not line.endswith("\n"):
                raise ValueError("speech command exceeds line limit")
            command = json.loads(line)
            if set(command) == {"start"} and type(command["start"]) is int and command["start"] >= 0:
                speech.start(command["start"])
            elif (set(command) == {"stop", "transcribe"}
                  and type(command["stop"]) is int and command["stop"] >= 0
                  and type(command["transcribe"]) is bool):
                speech.stop(command["stop"], command["transcribe"])
            else:
                raise ValueError("invalid speech command")
    finally:
        speech.close()


if __name__ == "__main__":
    from faster_whisper import WhisperModel
    model = WhisperModel(
        os.environ.get("TEHUTI_SPEECH_MODEL", "small"),
        device=os.environ.get("TEHUTI_SPEECH_DEVICE", "cuda"),
        compute_type="int8",
    )
    speech = Speech(model)
    def terminate(_signal, _frame):
        raise SystemExit(0)
    signal.signal(signal.SIGTERM, terminate)
    run(sys.stdin, speech)
