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

Install pw-record and pw-dump through your OS's PipeWire package. Create an isolated Python
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

Each press records from every PipeWire capture source at once (monitors are
excluded). On release the sidecar transcribes the source with the highest
peak and logs the winner once per released capture session to the Tehuti log. A source that fails
to start is skipped. The Input page of Tehuti's settings writes
`$XDG_CONFIG_HOME/tehuti/speech-source`: `all` (or no file) keeps this,
`auto` records the default source, and a PipeWire node name records that
source while it is listed. `TEHUTI_SPEECH_SOURCE` pins one source by its
PipeWire name and overrides both.

## Transport and limits

Commands and replies are one JSON object per line, capped at 64 KiB:

```json
{"start":0}
{"stop":0,"transcribe":true}
{"ready":true}
{"session":0,"text":"confirm"}
{"session":1,"error":"model failed"}
{"session":2,"error":"silent"}
```

An `error` reply shows SPEECH FAILED instead of NO SPEECH MAPPING. The
`silent` error shows MIC SILENT: the sidecar sends it without transcribing
when the capture is empty or its peak sample amplitude is at most 64, such as
the digital silence of a headset disconnected from its base. Each finished
capture logs its source, byte size and peak only to the Tehuti log at INFO. The compositor logs at
INFO each mapped key or text it delivers, naming the focused window's app id
and title.

`transcribe:false` discards audio. Session ids are nonnegative integers.
The core accepts one transcript for the current released session; stale,
malformed, NUL-containing and unknown-field replies are discarded. Pipe work
is bounded and nonblocking. Sidecar exit restarts after one second;
initialization has a 120-second deadline. Each sidecar owns a separate process
group; teardown kills the group so a crashed sidecar leaves no recorder running. Each recording keeps at most sixty
seconds of audio. One transcription runs at a time, with at most one waiting transcription. EOF cancels capture.

Synthetic tests need no microphone or model. Real audio quality and CUDA setup
must be checked with the intended microphone before the host handoff.

`TEHUTI_SPEECH_TAIL_MS` sets capture time after release per source, in whole
milliseconds: `remote-mic=1200,default=150`. Source names match PipeWire node
names. Unlisted sources use `default`, whose built-in value is 150 ms. Each
source stops at its own deadline; transcription waits for the longest active
tail and chooses the strongest recording, including audio received during the
tail. A new press or cancellation discards the pending tail. Values must be
between 0 and 60000 ms. Invalid entries warn and use the default; an invalid
`default` uses the built-in value.

## Vocabulary and prefix commands

The compositor sends `{"vocabulary":["Orchid","Open browser"]}` before capture,
on successful context reload, and after a sidecar restart. Candidates include all
loaded speech phrases, immediate folders in `~/Projects`, skill folders in
`~/.agents/skills` and `~/.claude/skills`, and the base `Name` and `GenericName`
from installed `.desktop` files in the XDG data directories. Add personal names,
one per line, to `$XDG_CONFIG_HOME/tehuti/vocabulary.txt` (default
`~/.config/tehuti/vocabulary.txt`); blank lines and lines starting with `#` are
ignored. Editing this file triggers the context reload watcher. Folder and
application changes are picked up on the next successful reload.

The sidecar deduplicates candidates and admits whole entries while they fit
within 64 Whisper tokens, in order: personal names, projects, skills, phrases,
applications. The candidate message is capped at 30 KiB. Every application is a
candidate, including applications without a context. Oversized entries are
skipped. Transcription, live preview and the VAD retry use the same hotwords.
`initial_prompt` is omitted: the measured combined priming exceeded Whisper's
decoder position limit. The short hotword budget bounds decoder prompt work.

Contexts can map a prefix followed by a free tail:

```toml
[speech]
"Invoke <name>" = { text = "/{name} " }
"Work on <project>" = { text = "cd -- {project}" }
```

`Invoke review-code.` types `/review-code `. Fixed mappings take precedence.
`Work on blue bird.` resolves the unique closest folder in `~/Projects`, at
similarity 0.8 or better with a lead of at least 0.05, and substitutes its
absolute path as a quoted shell argument. Missing or ambiguous projects do not
match. These templates apply only where configured; the terminal example maps
`Work on <project>`. Neither template submits the resulting text automatically.

`Type slash a one Enter Backspace.` emits `/`, A, 1, Enter and Backspace in
order. Symbol names include brackets, braces, parentheses, quotes and shell
punctuation; letters, digits and key names are accepted. An unknown token rejects
the whole sequence with `UNKNOWN SPOKEN KEY`. Exact context mappings and plugin
routing take precedence. The on-screen keyboard keeps its existing dictation
routing.
