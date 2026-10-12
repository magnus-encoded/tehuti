# Speech sidecar

The core owns push-to-talk. Hold the controller's Share (KEY_RECORD) button to
record; release it to transcribe. Focus changes, controller disconnects, Quit
and sidecar failure cancel the session. A restarted sidecar waits for a new
press after release. Guide held for three seconds still quits Tehuti.

The sidecar uses voice-cmd's pw-record capture settings (16 kHz mono s16) and
faster-whisper final transcription settings (small, CUDA int8, English, beam 5).
It does not read the controller or send input to applications. No service is
stopped automatically. The host voice-cmd handoff is an owner-controlled step:
keep a working recovery path before giving Tehuti controller ownership.

## Run

Install pw-record through your OS's PipeWire package. Create an isolated Python
venv outside voice-cmd's environment:

```sh
python3 -m venv /tmp/tehuti-speech-venv
/tmp/tehuti-speech-venv/bin/pip install -r sidecars/speech/requirements.txt
```

Create an executable wrapper whose paths point to this checkout and venv:

```sh
#!/bin/sh
exec /tmp/tehuti-speech-venv/bin/python /absolute/path/sidecars/speech/speech.py
```

Set `TEHUTI_SPEECH_SIDECAR` to the wrapper's absolute path when launching
Tehuti. Without it speech is disabled. `TEHUTI_SPEECH_MODEL` selects the model;
`TEHUTI_SPEECH_DEVICE=cpu` selects CPU inference. CUDA requires the NVIDIA
cuBLAS and cuDNN libraries expected by faster-whisper. Model initialization
may download model weights. The sidecar emits readiness after initialization.

## Transport and limits

Commands and replies are one JSON object per line, capped at 64 KiB:

```json
{"start":0}
{"stop":0,"transcribe":true}
{"ready":true}
{"session":0,"text":"confirm"}
```

`transcribe:false` discards audio. Session ids are nonnegative integers.
The core accepts one transcript for the current released session; stale,
malformed, NUL-containing and unknown-field replies are discarded. Pipe work
is bounded and nonblocking. Sidecar exit restarts after one second;
initialization has a 120-second deadline. Each sidecar owns a separate process
group; teardown kills the group so a crashed sidecar leaves no recorder running. Each recording keeps at most sixty
seconds of audio. One transcription runs at a time, with at most one waiting transcription. EOF cancels capture.

Synthetic tests need no microphone or model. Real audio quality and CUDA setup
must be checked with the intended microphone before the host handoff.
