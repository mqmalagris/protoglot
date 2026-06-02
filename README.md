# protoglot

[![CI](https://github.com/mqmalagris/protoglot/actions/workflows/ci.yml/badge.svg)](https://github.com/mqmalagris/protoglot/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue)](#license)

**A local-first, git-friendly API client that speaks REST, GraphQL, SOAP,
WebSocket, and gRPC — with a first-class CLI for CI/CD and a native desktop app.**
Written in Rust.

> The portability of [Bruno](https://www.usebruno.com/) (plain-text collections
> in git, a real CLI) with the protocol breadth of Postman (including gRPC and
> SOAP) — no account, no cloud, no lock-in.

```sh
protoglot new myapi      # scaffold a collection
protoglot test myapi     # run it — exits non-zero if anything fails
```

## Why

- **Local-first & git-friendly.** Collections are plain TOML files — one request
  per file in a folder tree. Diff them, review them in PRs, branch them. No
  exported JSON blobs, no account, no sync service.
- **CLI-first.** The exact collection you use at your desk runs in CI with
  JUnit/TAP output and proper exit codes — a free, multiprotocol equivalent of
  Newman.
- **Truly multiprotocol.** REST, GraphQL, SOAP, WebSocket, and *dynamic* gRPC all
  share one collection format and one assertion engine.
- **One core, thin shells.** All the logic lives in a Rust `core` library; the
  CLI and desktop app are thin layers over it, so what runs on your machine is
  literally the same engine that runs in CI.

| | Git-friendly | First-class CLI | Broad protocols | Local-first |
|---|---|---|---|---|
| Postman | ❌ | Newman (paid/limited) | ✅ | ❌ |
| Bruno | ✅ | ✅ | ⚠️ REST/GraphQL | ✅ |
| **protoglot** | ✅ | ✅ | ✅ (incl. gRPC/SOAP) | ✅ |

> **Status:** early — `v0.1.0`. The protocols, assertions, auth, and CI tooling
> described below work and are covered by tests; some refinements are still in
> flight.

## Install

**Prebuilt binary** — download the archive for your platform from
[Releases](https://github.com/mqmalagris/protoglot/releases), extract, and put
`protoglot` on your `PATH`. Every release carries SLSA build provenance
([verify it](#releases--provenance)).

**With Cargo:**

```sh
cargo install --git https://github.com/mqmalagris/protoglot protoglot-cli
```

**From source:**

```sh
git clone https://github.com/mqmalagris/protoglot
cd protoglot
cargo build --release -p protoglot-cli   # → target/release/protoglot
```

## Quickstart

```sh
protoglot new myapi
protoglot test myapi
```

```
✓ Get example [rest] 200 (47ms)
    ✓ status == 200
    ✓ jsonpath $.title

1 passed, 0 failed, 0 errored
```

`protoglot new` writes a runnable sample collection; edit it, add your requests,
point it at your API.

## Collections

A collection is a directory of TOML files — one request per file — in whatever
folder structure you like:

```
myapi/
├── protoglot.toml          # collection config + variables
├── environments/
│   ├── local.toml
│   └── staging.toml
└── users/
    ├── get-user.toml
    └── create-user.toml
```

A REST request:

```toml
name = "Get user"
method = "GET"
url = "{{baseUrl}}/users/{{userId}}"

[headers]
Authorization = "Bearer {{token}}"

[[assertions]]
type = "status"
equals = 200

[[assertions]]
type = "jsonpath"
path = "$.id"
exists = true
```

Variables resolve with precedence **`--var` > environment > collection**. Dynamic
values are available too: `{{$uuid}}`, `{{$timestamp}}`, and `{{$secret:NAME}}`
(read from the environment, never written to disk).

## Assertions & capture

Assertion types: `status`, `jsonpath`, `xpath` (with namespace registration),
`header`, `response_time`, `body_contains`, and `schema` (JSON Schema —
[contract testing](#contract-testing)).

`[[capture]]` pulls a value out of a response into the run scope so later
requests can use it — auth chaining without any scripting:

```toml
[[capture]]
var = "authToken"
jsonpath = "$.token"
```

## Protocols

**GraphQL** — a non-empty `errors` array fails the request even on HTTP 200:

```toml
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

**SOAP** — XML envelope with a namespace-aware `xpath` assertion (`<Fault>` ⇒
failure):

```toml
kind = "soap"
name = "GetRate"
url = "{{soapHost}}/CurrencyService.asmx"
soap_action = "http://tempuri.org/GetRate"
body = """<soap:Envelope ...>...</soap:Envelope>"""
[[assertions]]
type = "xpath"
path = "//t:GetRateResult"
exists = true
[assertions.namespaces]
t = "http://tempuri.org/"
```

**WebSocket** — a scriptable send/expect roteiro (works in CI and the desktop):

```toml
kind = "websocket"
name = "Echo socket"
url = "wss://{{wsHost}}/echo"
[[steps]]
send = '{"type":"ping"}'
[[steps]]
expect_contains = "pong"
timeout_ms = 2000
```

**gRPC** — *dynamic* invocation, no codegen: descriptors come from a
runtime-compiled `.proto` **or** server reflection. The reply is converted to
JSON so the usual assertions apply.

```toml
kind = "grpc"
name = "GetUser"
target = "{{grpcHost}}:50051"
service = "user.v1.UserService"
method = "GetUser"
proto = "./protos/user.proto"      # or omit and set: schema = "reflection"
[message]
id = "{{userId}}"
[[assertions]]
type = "jsonpath"
path = "$.name"
exists = true
```

## Auth

Add an `[auth]` block to a request. Header schemes (`bearer`, `basic`,
`oauth2_client_credentials`) work on any HTTP protocol; `aws_sigv4` request
signing and `mtls` client certificates apply to REST.

```toml
[auth]
type = "bearer"
token = "{{$secret:api_token}}"
```

```toml
[auth]
type = "oauth2_client_credentials"
token_url = "{{idp}}/oauth/token"
client_id = "{{clientId}}"
client_secret = "{{$secret:client_secret}}"
scopes = ["api.read", "api.write"]
```

```toml
[auth]
type = "aws_sigv4"
access_key_id = "{{AWS_ACCESS_KEY_ID}}"
secret_access_key = "{{$secret:aws_secret}}"
region = "us-east-1"
service = "execute-api"
```

## Testing features

**Data-driven** — run a request once per row of a CSV/JSON dataset; columns
become variables:

```toml
name = "Get user"
url = "{{baseUrl}}/users/{{id}}"
[data]
file = "users.csv"     # format inferred from the extension (csv | json)
```

**Contract testing** — validate the JSON response against a JSON Schema; catches
breaking changes (missing/renamed fields, wrong types) that point assertions
miss:

```toml
[[assertions]]
type = "schema"
file = "schemas/user.json"   # or an inline [assertions.inline] schema
```

**Snapshot testing** — record a response and diff it on later runs. Snapshots are
versioned `.snap` files (canonical JSON), so regressions show up in your diff:

```toml
[snapshot]   # presence enables it
```

First run writes `__snapshots__/<request>.snap`; commit it. Re-record with
`protoglot test … --update-snapshots`.

## Scripting

For the cases declarative config can't cover, add a `pre_script` (runs before the
request) or `post_script` (runs after) — pure-Rust JavaScript via
[boa](https://github.com/boa-dev/boa):

```toml
pre_script = "pg.set('id', 2);"
post_script = """
pg.assert('ok', pg.response.status === 200);
pg.set('title', pg.response.json.title);
"""
```

`pg.get/set` read and write run variables; in `post_script`,
`pg.response.{status,body,json}` exposes the response and `pg.assert(name, cond)`
adds a checked assertion.

## CI/CD

The whole point of the CLI: run the same collection in your pipeline.

```yaml
- run: protoglot test ./api-tests --env ci --reporter junit > junit.xml
- uses: mikepenz/action-junit-report@v5
  with: { report_paths: junit.xml }
```

The exit code is non-zero if any assertion fails, so the build breaks on its own.

## CLI reference

```
protoglot new  <name>                              scaffold a runnable collection
protoglot run  <path>                              execute a request / folder / collection
protoglot test <path>                              same, intended for CI
protoglot codegen <file> --as curl|fetch|reqwest   export a request as a snippet
protoglot lint <path>                              flag hardcoded secrets
```

Installs as both `protoglot` and the short alias `pglot` (`pglot test ./api`).

`run` / `test` flags: `--env <name>`, `--reporter pretty|json|junit|tap`,
`--var key=value` (repeatable), `--bail`, `--timeout <secs>`,
`--concurrency <N>`, `--watch` (re-run on change), `--http-version auto|1|2`,
`--update-snapshots`.

There's a runnable example collection in [`examples/demo`](./examples/demo).

## Desktop app

A native [egui](https://github.com/emilk/egui) GUI — all Rust, no web view —
that drives the same engine as the CLI: pick a collection, run it, browse
results, and edit request source with TOML highlighting and save-back.

```sh
cargo run -p protoglot-desktop
```

## Releases & provenance

Tagging `vX.Y.Z` builds the `protoglot` CLI for Linux, macOS (x86_64 + arm64),
and Windows, publishes a GitHub Release with the archives, and attaches **SLSA
build provenance** (Sigstore-signed, via GitHub artifact attestations). Verify a
downloaded artifact:

```sh
gh attestation verify protoglot-vX.Y.Z-<target>.tar.gz --repo mqmalagris/protoglot
```

## Project layout

```
crates/
  format/    on-disk collection format (parse/serialize; no runtime)
  core/      the engine: protocols, runner, environment, assertions, reporting
  cli/       the `protoglot` binary
  desktop/   native egui app (thin view over core)
```

Build and test everything with `cargo build` / `cargo test`. (The desktop crate
needs GUI system libraries; on headless machines, `cargo test --workspace
--exclude protoglot-desktop`.)

## License

Dual-licensed under either [MIT](./LICENSE-MIT) or [Apache-2.0](./LICENSE-APACHE)
at your option.
