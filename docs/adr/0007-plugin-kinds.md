# 7. Plugin permissions follow the plugin's kind

## Status

Accepted

## Context

Extends [0003](0003-plugin-protocol.md). Until now every plugin served named applications. Some plugins extend Tehuti itself and must hear the user in any window, for example addressing an assistant by name.

## Decision

A plugin's manifest declares exactly one **kind**. The kind sets which permissions the plugin may request. The user **grants** them when enabling the plugin, and grants them again when an update changes the plugin's kind or scope.

- **Application:** two-way communication with the applications the manifest names, which can be more than one. It receives input and primes speech only while one of those applications has focus.
- **Verb:** extends Tehuti itself. In any window, it receives the speech that follows one of its own verbs, and it primes speech only for those verbs.

A plugin that needs both behaviours ships as two plugins.

The manifest fields, routing and priming budget are in `docs/protocol/README.md`.
