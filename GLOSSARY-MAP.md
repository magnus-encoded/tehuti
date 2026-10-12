# Glossary map

Tehuti has several bounded contexts, each with its own `GLOSSARY.md`. Read the ones relevant to the work. A glossary is created when its first term is settled, so a missing file means no terms are settled yet.

| Context | Glossary | Covers |
| --- | --- | --- |
| Core | `crates/core/GLOSSARY.md` | The pure core step: input routing, layers, state machines. No Smithay, no I/O |
| Compositor | `crates/compositor/GLOSSARY.md` | The `tehuti` binary: Smithay, window model, focus, overlays, input sources, applying actions |
| Protocol | `crates/protocol/GLOSSARY.md` | Plugin protocol: events, actions, manifests, `context.toml` |
| Speech | `sidecars/speech/GLOSSARY.md` | Speech sidecar: push-to-talk, transcription, hotwords |

The compositor crate depends on core and protocol; core depends only on protocol.

System-wide decisions live in `docs/adr/`; context decisions in `crates/<context>/docs/adr/` (or `sidecars/<context>/docs/adr/`).
