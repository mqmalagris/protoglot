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
- **Auth** (Phase 3): `bearer`, `basic`, `oauth2_client_credentials` (header
  schemes, any HTTP protocol), `aws_sigv4` request signing and `mtls` client
  certs (REST). OAuth2 authorization-code + PKCE is the remaining follow-up.
- **Data-driven** (Phase 7): `[data]` iterates a request over each row of a
  CSV/JSON file; columns/keys become variables for that row.
- **Contract testing** (Phase 8): the `schema` assertion validates the JSON
  response against a JSON Schema (`file` or `inline`) — catches breaking
  changes point assertions miss. OpenAPI-driven validation is a follow-up.
- **Snapshot testing** (Phase 9): `[snapshot]` records the response on first run
  to a versioned `.snap` file and diffs it on later runs; `--update-snapshots`
  re-records. Git-first regression detection.

The **desktop app** (Phase 4, Tauri v2 + Svelte 5) is scaffolded in
[`desktop/`](./desktop) as a thin shell over `core` — run with `bun run tauri dev`.

gRPC, WebSocket, and JS scripting are **stubs / not yet built** — see the
roadmap (§11) in the spec.

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

### Auth (Phase 3)

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
# session_token = "{{AWS_SESSION_TOKEN}}"   # optional
```
```toml
[auth]
type = "mtls"
pem = "./client-bundle.pem"   # or: cert = "...", key = "..."
```

### Data-driven (Phase 7)

```toml
name = "Get user"
url = "{{baseUrl}}/users/{{id}}"

[data]
file = "users.csv"     # relative to this request; format inferred (csv|json)

[[assertions]]
type = "status"
equals = 200
```
`users.csv` (header row = variable names):
```csv
id,name
1,ada
2,grace
```
Runs the request once per row (`Get user [row 1]`, `[row 2]`, …), with `{{id}}`
and `{{name}}` bound from each row. JSON datasets are an array of objects.

### Contract testing (Phase 8)

```toml
[[assertions]]
type = "schema"
file = "schemas/user.json"   # JSON Schema, relative to the request
```
Or inline:
```toml
[[assertions]]
type = "schema"
[assertions.inline]
type = "object"
required = ["id", "name"]
```
A failed schema reports the offending path, so it surfaces breaking changes
(missing/renamed fields, wrong types) across commits.

### Snapshot testing (Phase 9)

```toml
name = "Get user"
url = "{{baseUrl}}/users/1"

[snapshot]                     # presence enables it; optional: file = "..."
```
First run writes `__snapshots__/<request>.snap` (canonical JSON, sorted keys);
commit it. Later runs diff against it and fail on drift. Re-record with
`protoglot test ... --update-snapshots`.

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
protoglot new <name>                              # scaffold a runnable collection
protoglot run  <path> [--env <name>] [--var k=v]...
protoglot test <path> --reporter junit > results.xml
protoglot codegen <file> --as curl|fetch|reqwest  # export a request
protoglot lint <path>                             # flag hardcoded secrets
```

`protoglot new myapi && protoglot test myapi` is green out of the box (the
sample request hits jsonplaceholder).

**run / test flags:** `--env <name>`, `--reporter pretty|json|junit|tap`,
`--bail`, `--var k=v` (repeatable), `--timeout <secs>` (default 30, 0 disables),
`--concurrency <N>` (parallel; captures don't propagate when > 1),
`--watch` (re-run on `.toml` change), `--http-version auto|1|2`.
Exit code ≠ 0 if any assertion fails — CI breaks the build.

Reporters: `pretty` (default), `json`, `junit`, `tap`. Exit code is non-zero if
any assertion fails — so CI breaks the build.

There is a runnable example collection in [`examples/demo`](./examples/demo).
