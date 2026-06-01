# protoglot

Local-first, git-friendly multiprotocol API client (Rust + Tauri + CLI).

> Bruno's git+CLI portability × Postman's protocol breadth.

See [`protoglot-spec.md`](./protoglot-spec.md) for the full design.

## Status

**Phase 0 + 1** implemented: workspace, TOML collection format, `core` runner,
REST execution, declarative assertions, and the `protoglot` CLI with
`pretty`/`json`/`junit`/`tap` reporters.

GraphQL, SOAP, gRPC, WebSocket, scripting, and the desktop app are **stubs /
not yet built** — see the roadmap (§11) in the spec.

## Workspace

```
crates/
  format/   # pure parse/serialize of the on-disk collection (serde + toml)
  core/     # domain: protocols, runner, environment, assertions, report
  cli/      # `protoglot` binary (clap)
desktop/     # Tauri v2 + Svelte 5 (scaffolded later)
```

`core` depends on `format` (matches the §3 layering). `format` carries no
execution runtime so it can be reused by external tools (editor plugins, etc.).

## Build & test

```sh
cargo build
cargo test
```

## CLI

```sh
protoglot run <path> [--env <name>] [--var k=v]...
protoglot test <path> --reporter junit > results.xml
```

Reporters: `pretty` (default), `json`, `junit`, `tap`. Exit code is non-zero if
any assertion fails — so CI breaks the build.

There is a runnable example collection in [`examples/demo`](./examples/demo).
