# Architecture

Architecture documentation for lean-rust: a global crate-dependency / class
overview plus per-layer class and sequence diagrams.

> ⚠️ This documentation is work in progress and tracks a WIP codebase. Diagrams
> may lag the source until each layer is filled in.

## Layout

| File | Contents |
| ---- | -------- |
| [`global-class.md`](global-class.md) | Global class / crate-dependency diagram across all layers |
| `layers/domain.md` | Domain / Data layer — `types`, `ssz`, `config` (planned) |
| `layers/protocol.md` | Protocol / Consensus — `protocol`, `forkchoice` (planned) |
| `layers/storage.md` | Storage — `storage` (planned) |
| `layers/networking.md` | Networking — `networking`, `runtime/p2p`, `runtime/p2p-rpc` (planned) |
| `layers/runtime.md` | Runtime / Services — `runtime/core`, `runtime/chain`, `runtime/sync`, `runtime/duties`, `runtime/api` (planned) |
| `layers/application.md` | Application / Entry — `node`, `lean-cli`, `bin/lean-rust` (planned) |

PlantUML sources live in [`diagrams/`](diagrams/) next to their rendered `.svg`.

## Layer map

| Layer | Crates (package name) |
| ----- | --------------------- |
| Domain / Data | `types`, `ssz`, `config` |
| Protocol / Consensus | `protocol`, `forkchoice` |
| Storage | `storage` |
| Networking | `networking` (lean-wire), `runtime/p2p` (lean-p2p-host), `runtime/p2p-rpc` |
| Runtime / Services | `runtime/core` (lean-core), `runtime/chain` (lean-chain), `runtime/sync` (lean-sync), `runtime/duties` (lean-duties), `runtime/api` (lean-api) |
| Cross-cutting | `observability` (lean-observability) |
| Application / Entry | `node`, `lean-cli`, `bin/lean-rust` |

Dependencies flow upward: `domain → protocol → storage / networking → runtime → application`.
The `domain` layer pulls only `core`/`std`/`alloc` plus serialization, never
infrastructure (see `.claude/rules/architecture.md`).

## Diagram conventions

- **Tool:** PlantUML, rendered to committed SVG so the diagrams display on GitHub
  (GitHub does not render `.puml` source inline).
- **Class diagrams** are exhaustive at the per-layer level: every public
  `struct` / `enum` / `trait` with its fields, variants, and method signatures.
  The global diagram shows only the primary aggregate type per crate plus
  cross-crate dependency edges (transitive edges omitted for readability).
- **Sequence diagrams** capture representative runtime flows per layer.

## Regenerating diagrams

PlantUML is required (`brew install plantuml`, which pulls OpenJDK; Graphviz is
a dependency and is used for class-diagram layout). Render all sources to SVG:

```sh
plantuml -tsvg docs/architecture/diagrams/*.puml
```

Render a single diagram:

```sh
plantuml -tsvg docs/architecture/diagrams/global-class.puml
```

Commit both the edited `.puml` and the regenerated `.svg` in the same change so
the rendered output never drifts from its source.
