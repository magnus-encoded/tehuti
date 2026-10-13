#!/usr/bin/env python3
"""Capture on core commands; return transcription without delivering input."""
import json
import os
import signal
import subprocess
import sys
import tempfile
import threading
import wave

MAX_LINE = 65536
MAX_AUDIO_SECONDS = 60


def reply(value):
    print(json.dumps(value), flush=True)


class Speech:
    def __init__(self, model):
        self.model = model
        self.capture = None
        self.clip = None
        self.session = None
        self.model_lock = threading.Lock()
        self.transcriptions = threading.BoundedSemaphore(2)

    def start(self, session):
        if self.capture is not None:
            return
        self.clip = tempfile.TemporaryFile()
        self.capture = subprocess.Popen(
            ["pw-record", "--raw", "--format", "s16", "--rate", "16000", "--channels", "1", "-"],
            stdout=subprocess.PIPE, stderr=sys.stderr,
        )
        self.session = session
        capture, clip = self.capture, self.clip

        def drain():
            remaining = MAX_AUDIO_SECONDS * 16000 * 2
            while remaining:
                chunk = capture.stdout.read(min(4096, remaining))
                if not chunk:
                    break
                clip.write(chunk)
                remaining -= len(chunk)
            if remaining == 0 and capture.poll() is None:
                capture.send_signal(signal.SIGINT)

        self.reader = threading.Thread(target=drain, daemon=True)
        self.reader.start()

    def stop(self, session, transcribe):
        if self.capture is None or session != self.session:
            return
        capture, clip = self.capture, self.clip
        self.capture = self.clip = self.session = None
        if capture.poll() is None:
            capture.send_signal(signal.SIGINT)
        try:
            capture.wait(timeout=3)
        except subprocess.TimeoutExpired:
            capture.kill()
            capture.wait()
        self.reader.join(timeout=3)
        if self.reader.is_alive():
            capture.stdout.close()
            clip.close()
            return
        capture.stdout.close()
        if not transcribe:
            clip.close()
            return
        if not self.transcriptions.acquire(blocking=False):
            clip.close()
            reply({"session": session, "text": ""})
            return

        def finish():
            try:
                clip.seek(0)
                audio = clip.read()
                if audio:
                    with tempfile.NamedTemporaryFile(suffix=".wav") as recording:
                        with wave.open(recording.name, "wb") as output:
                            output.setnchannels(1)
                            output.setsampwidth(2)
                            output.setframerate(16000)
                            output.writeframes(audio)
                        with self.model_lock:
                            segments, _ = self.model.transcribe(
                                recording.name, beam_size=5, language="en", vad_filter=True,
                            )
                            text = " ".join(segment.text.strip() for segment in segments).strip()
                else:
                    text = ""
                reply({"session": session, "text": text})
            except Exception as error:
                print(f"speech transcription failed: {error}", file=sys.stderr)
                reply({"session": session, "error": str(error)})
            finally:
                clip.close()
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
