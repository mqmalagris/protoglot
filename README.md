# protoglot

Local-first, git-friendly multiprotocol API client (Rust + Tauri + CLI).

> Bruno's git+CLI portability × Postman's protocol breadth.

See [`protoglot-spec.md`](./protoglot-spec.md) for the full design.

## Status

**Phase 0 + 1 + 2** implemented:

- Workspace, TOML collection format, `core` runner, the `protoglot` CLI with
  `pretty`/`json`/`junit`/`tap` reporters and CI exit codes (Phase 0–1).
- REST execution + declarative assertions: `status`, `jsonpath`, `xpath`,
  `header`, `response_time`, `body_contains` (Phase 1–2).
- **GraphQL** (POST `{query, variables}`; non-empty `errors` ⇒ failure even on
  HTTP 200) and **SOAP** (XML envelope, `SOAPAction`; `<Fault>` ⇒ failure),
  reusing the REST/HTTP layer (Phase 2).
- **`[[capture]]`** — pull a value (jsonpath/xpath) from a response into the run
  scope for later requests; covers auth-chaining without a JS engine (Phase 2).

gRPC, WebSocket, JS scripting, and the desktop app are **stubs / not yet built**
— see the roadmap (§11) in the spec.

### Phase 2 syntax

```toml
# GraphQL
kind = "graphql"
name = "Fetch user"
url = "{{baseUrl}}/graphql"
query = "query($id: ID!) { user(id: $id) { id name } }"
[variables]
id = "{{userId}}"
[[assertions]]
type = "jsonpath"
path = "$.data.user.name"
exists = true
```

```toml
# SOAP — with a namespace-aware xpath assertion
kind = "soap"
name = "GetRate"
url = "{{soapHost}}/CurrencyService.asmx"
soap_action = "http://tempuri.org/GetRate"
body = """<soap:Envelope ...>...</soap:Envelope>"""
[[assertions]]
type = "xpath"
path = "//t:GetRateResult"
exists = true
[assertions.namespaces]      # prefixes must be registered or the query won't match
t = "http://tempuri.org/"
```

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
