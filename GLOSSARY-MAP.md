# Glossary map

Tehuti has several bounded contexts, each with its own `GLOSSARY.md`. Read the ones relevant to the work. A glossary is created when its first term is settled, so a missing file means no terms are settled yet.

| Context | Glossary | Covers |
| --- | --- | --- |
| Core | `crates/core/GLOSSARY.md` | Compositor, window model, focus, overlays, input routing |
| Protocol | `crates/protocol/GLOSSARY.md` | Plugin protocol: events, actions, manifests, `context.toml` |
| Speech | `crates/speech/GLOSSARY.md` | Speech sidecar: push-to-talk, transcription, hotwords |

System-wide decisions live in `docs/adr/`; context decisions in `crates/<context>/docs/adr/`.
