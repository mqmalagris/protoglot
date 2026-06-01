# protoglot desktop (Tauri v2 + Svelte 5)

**Not yet scaffolded.** Phase 4 (§11). This directory is reserved for the
desktop app — a thin layer over `protoglot-core` (the same crate the CLI uses):

- `src-tauri/` — Rust backend exposing `#[tauri::command]` wrappers that call `core`.
- `src/` — Svelte 5 + Vite frontend; CodeMirror 6 editor; Tauri events for streaming.

It is intentionally **outside the Cargo workspace** so `cargo build`/`cargo test`
at the repo root don't require the Tauri toolchain or Node. Scaffold later with:

```sh
pnpm create tauri-app@latest    # choose Svelte + TypeScript + Vite
```
