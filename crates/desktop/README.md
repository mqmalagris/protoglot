# protoglot desktop (egui)

A native, all-Rust GUI over `protoglot-core` — **no WebView, no JS, no IPC**.
The window calls the same `Runner::run_all` the CLI uses, directly in-process
(§2 core-first). The frontend is a thin, disposable view; all logic lives in
`core`, so this can be swapped for a polished stack later without touching it.

## Run

```sh
cargo run -p protoglot-desktop
```

Pick a collection folder (the 📁 button, or type a path — try `examples/demo`
or one from `protoglot new`), optionally an environment, then **Load** →
**Run all**. Requests list on the left with pass/fail dots; selected request
source and results on the right.

## Layout

- `src/main.rs` — the whole app: an `eframe::App` that lists requests, reads
  source, and runs the collection on a background tokio task (results return
  over a channel; the window repaints when done).

## Status / not done

- **egui chosen over Tauri+Svelte and Slint** (see project DEFERRED notes):
  devtool sweet spot, 100% Rust, MIT/Apache license match, simplest async wiring.
- **Editable source + save-back** — edit a request's TOML in place and **Save**
  (or **Ctrl/Cmd+S**) writes it back to disk (row name refreshes). Switching
  requests or closing the window with unsaved edits prompts (Save / Discard /
  Cancel) instead of dropping them.
- **TOML syntax highlighting** — a small dependency-free highlighter
  (`src/highlight.rs`): comments, strings (incl. multi-line `"""`), `[sections]`,
  keys. No syntect/onig (cross-compile clean). TOML-only for now.
- **Compiles; not yet run on a display** (built headless). First
  `cargo run -p protoglot-desktop` on a desktop is the smoke test.
- **Streaming** (WS/gRPC) — arrives with Phases 5/6.
