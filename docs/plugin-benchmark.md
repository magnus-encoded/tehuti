# Plugin stdio benchmark

The weekly `plugin benchmark` workflow runs beside `mutants`, on Mondays at
03:00 UTC. It also supports manual runs. It needs no compositor session, display,
device or microphone.

Run the measurement with:

```sh
cargo run --release --locked -p tehuti-protocol --bin plugin-benchmark > plugin-benchmark.json
```

The executable starts another copy of itself as the reference plugin, using real
stdin and stdout pipes and newline-delimited JSON. It negotiates protocol v0,
warms up each workload with 100 exchanges, then records 10,000 exchanges per
workload. `--samples N` changes the recorded count for smoke tests.

The button workload sends East presses and receives an empty action response.
The axis workload sends alternating signed axis values using v0's `adjuster`
notice, with the name `axis`, and waits for an empty `actions` notice. Protocol
v0 has no raw axis event method. This fixture measures its supported analogue
adjustment path without adding a wire method.

The host uses the same `serde_json::to_vec` encoder and `parse_message` decoder
as the compositor's plugin session. The round trip starts before encoding and
ends after decoding the reply. JSON time is the sum of host encoding and
host decoding only. Pipe writes, flushes, reads, scheduling and reference-plugin
work belong to the round trip. Process startup, negotiation, warmup and report
formatting are excluded. This measures the stdio exchange, without the display
event loop or input routing.

`scripts/plugin-benchmark.py` computes nearest-rank p99 independently for round
trip, JSON time and the paired JSON-time/round-trip ratio. The ratio is computed
for each exchange before taking p99; it is not a ratio of separate percentiles.
Both workloads are assessed independently. Raw samples are retained as a CI
artifact. Results and thresholds appear in the job summary.

[scripts/plugin-benchmark-thresholds.json](../scripts/plugin-benchmark-thresholds.json)
is the single threshold file. A strict JSON-share breach reopens #24 and comments
with the measurements. A strict round-trip breach with JSON share within its
threshold opens a `needs-triage` issue about the core fast path. If different
workloads breach different thresholds, both actions run. Exact equality passes.
Any breach fails the job. Only the `gh` CLI writes issues, authenticated by the
workflow's `GITHUB_TOKEN` through `GH_TOKEN`.

The owner can link the threshold file from ADR 0003.
