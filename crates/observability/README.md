# lean-observability

Tracing-subscriber setup for the runtime shell.

Verbosity → `EnvFilter` directive, the global subscriber install, and an
optional rolling file sink. No custom `FormatEvent` impl — the default
human-readable `tracing_subscriber::fmt()` output is the canonical shape.
`RUST_LOG` overrides the CLI-derived directive.

## Scope

- [`Verbosity`] / [`ParseVerbosityError`] — coarse level enum
  (`Error`..`Trace`) with a [`Verbosity::directive`] builder that silences
  known-noisy ecosystem crates.
- [`init_tracing`] — installs the global subscriber once (race-safe via an
  internal `OnceLock` claim); gates stderr ANSI on `io::IsTerminal`; warns
  once on a malformed `RUST_LOG`; optionally adds a non-blocking file sink.
- [`FileSink`] / [`LogRotation`](./src/init.rs) — directory + prefix +
  rotation policy for the file sink (default daily rotation,
  `<prefix>.<date>.log`).
- [`TracingGuard`] — RAII guard; drop at shutdown to flush the file writer.
- [`TracingInitError`] — init failure surface (`AlreadyInitialized`,
  `Install`, `CreateLogDir`, `FileAppender`).

[`Verbosity`]: ./src/verbosity.rs
[`ParseVerbosityError`]: ./src/verbosity.rs
[`Verbosity::directive`]: ./src/verbosity.rs
[`init_tracing`]: ./src/init.rs
[`FileSink`]: ./src/init.rs
[`TracingGuard`]: ./src/init.rs
[`TracingInitError`]: ./src/init.rs

## Tier and dependencies

Runtime-support crate. Depends on `tracing`, `tracing-subscriber`, and
`tracing-appender` only — no consensus or runtime-service imports. Consumed
by the `lean-rust` binary at startup.
