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
- **Compiles; not yet run on a display** (scaffolded headless). First
  `cargo run -p protoglot-desktop` on a desktop is the smoke test.
- **Read-only source view** for now (egui `code_editor`, non-interactive); a
  real editor + save-back is the next step.
- **No syntax highlighting** (the WebView's CodeMirror is what we gave up). An
  egui syntax-highlight layer can be added later.
- **Streaming** (WS/gRPC) — arrives with Phases 5/6.
