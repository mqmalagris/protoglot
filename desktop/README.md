# protoglot desktop (Tauri v2 + Svelte 5)

A thin GUI over `protoglot-core` — the **same** runner the CLI uses (§2
core-first). The Rust side (`src-tauri/src/main.rs`) is just three commands that
call `core`; all protocol/auth/assertion logic stays in the crate.

```
desktop/
├── package.json          # svelte 5 + vite + @tauri-apps/cli
├── index.html  vite.config.ts  svelte.config.js  tsconfig.json
├── src/                  # frontend
│   ├── App.svelte        # collection picker · request list · run · results
│   ├── lib/api.ts        # typed invoke() wrappers (mirror core's types)
│   └── main.ts  app.css
└── src-tauri/            # Rust backend (own Cargo workspace)
    ├── Cargo.toml        # depends on protoglot-core (path)
    ├── src/main.rs       # #[tauri::command] list_requests / read_request / run_collection
    ├── tauri.conf.json   capabilities/default.json  build.rs
```

## Run it (needs Node + Rust + a WebView)

```sh
cd desktop
pnpm install
pnpm tauri dev          # launches the app against the Vite dev server
```

Then point the path box at a collection (e.g. the repo's `examples/demo`, or one
made with `protoglot new`), **Load**, then **Run all**.

## Build a bundle

```sh
pnpm tauri icon path/to/logo.png   # generate src-tauri/icons/* (required to bundle)
pnpm tauri build
```

## Status / not yet done

- **Built and run on a real machine** — it was scaffolded headless, so treat the
  first `pnpm tauri dev` as the smoke test. The Rust commands mirror the CLI
  paths exactly, so behavior should match.
- **CodeMirror 6 editor** (spec §9) — the request source is shown read-only in a
  `<pre>` for now; the syntax-highlighted editor + save-back is the next step.
- **Streaming (WS/gRPC)** via Tauri events — arrives with Phases 5/6.
- **Icons** aren't committed; generate them before `tauri build`.
