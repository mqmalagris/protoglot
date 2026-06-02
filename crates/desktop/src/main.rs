// Hide the console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! protoglot desktop — a native egui app that is a thin view over `core` (§2).
//! No WebView, no IPC: the UI calls the same `Runner` the CLI uses, directly.
//! The async run happens on a background tokio task; results come back over a
//! channel and the window repaints.

use eframe::egui;
use protoglot_core::environment::Scope;
use protoglot_core::format::{self, VarMap};
use protoglot_core::report::{ExecStatus, ExecutionResult};
use protoglot_core::runner::{RunOptions, Runner};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};

mod highlight;
mod update;

struct RequestRow {
    name: String,
    kind: String,
    path: PathBuf,
}

/// A pending action blocked by unsaved edits.
#[derive(Clone, Copy)]
enum Pending {
    Switch(usize),
    Quit,
}

#[derive(Clone, Copy)]
enum Decision {
    Save,
    Discard,
    Cancel,
}

struct App {
    rt: tokio::runtime::Runtime,
    path: String,
    env: String,
    requests: Vec<RequestRow>,
    results: Vec<ExecutionResult>,
    selected: Option<usize>,
    source: String,
    dirty: bool,
    pending: Option<Pending>,
    status: String,
    running: bool,
    rx: Option<Receiver<Vec<ExecutionResult>>>,
    // Self-update state.
    update_msg: String,
    update_offer: Option<String>,
    update_busy: bool,
    update_rx: Option<Receiver<update::UpdateOutcome>>,
}

impl App {
    fn new() -> Self {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");
        Self {
            rt,
            path: String::new(),
            env: String::new(),
            requests: Vec::new(),
            results: Vec::new(),
            selected: None,
            source: String::new(),
            dirty: false,
            pending: None,
            status: String::new(),
            running: false,
            rx: None,
            update_msg: String::new(),
            update_offer: None,
            update_busy: false,
            update_rx: None,
        }
    }

    fn check_updates(&mut self, ctx: &egui::Context) {
        if self.update_busy {
            return;
        }
        self.update_busy = true;
        self.update_msg = "Checking for updates…".into();
        let (tx, rx) = channel();
        self.update_rx = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(update::check());
            ctx.request_repaint();
        });
    }

    fn install_update(&mut self, ctx: &egui::Context) {
        if self.update_busy {
            return;
        }
        self.update_busy = true;
        self.update_offer = None;
        self.update_msg = "Downloading update…".into();
        let (tx, rx) = channel();
        self.update_rx = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(update::install());
            ctx.request_repaint();
        });
    }

    fn poll_update(&mut self) {
        let outcome = self.update_rx.as_ref().and_then(|rx| rx.try_recv().ok());
        if let Some(outcome) = outcome {
            self.update_busy = false;
            self.update_rx = None;
            match outcome {
                update::UpdateOutcome::UpToDate => {
                    self.update_msg = "You're on the latest version.".into();
                    self.update_offer = None;
                }
                update::UpdateOutcome::Available(v) => {
                    self.update_msg = format!("Update available: v{v}");
                    self.update_offer = Some(v);
                }
                update::UpdateOutcome::Updated(v) => {
                    self.update_msg = format!("Updated to v{v} — restart protoglot to use it.");
                    self.update_offer = None;
                }
                update::UpdateOutcome::Failed(e) => {
                    self.update_msg = format!("Update failed: {e}");
                    self.update_offer = None;
                }
            }
        }
    }

    fn load(&mut self) {
        self.results.clear();
        self.selected = None;
        self.source.clear();
        match format::collect_requests(Path::new(&self.path)) {
            Ok(items) => {
                self.requests = items
                    .into_iter()
                    .map(|item| RequestRow {
                        name: item.request.name().to_string(),
                        kind: format!("{:?}", item.request.kind()).to_lowercase(),
                        path: item.path,
                    })
                    .collect();
                self.status = format!("{} request(s)", self.requests.len());
            }
            Err(e) => {
                self.requests.clear();
                self.status = e.to_string();
            }
        }
    }

    fn open(&mut self, idx: usize) {
        self.selected = Some(idx);
        self.source =
            std::fs::read_to_string(&self.requests[idx].path).unwrap_or_else(|e| e.to_string());
        self.dirty = false;
    }

    fn save(&mut self) {
        let Some(idx) = self.selected else { return };
        let path = self.requests[idx].path.clone();
        match std::fs::write(&path, &self.source) {
            Ok(()) => {
                self.dirty = false;
                // Refresh this row's name/kind in case `name`/`kind` changed,
                // without dropping any run results already on screen.
                if let Ok(req) = format::load_request(&path) {
                    self.requests[idx].name = req.name().to_string();
                    self.requests[idx].kind = format!("{:?}", req.kind()).to_lowercase();
                }
                self.status = format!("saved {}", path.display());
            }
            Err(e) => self.status = format!("save failed: {e}"),
        }
    }

    /// Open `idx`, but if the current request has unsaved edits, defer to a
    /// confirmation dialog instead of silently dropping them.
    fn request_open(&mut self, idx: usize) {
        if self.pending.is_some() {
            return; // a dialog is already up
        }
        if self.dirty && self.selected.is_some() && self.selected != Some(idx) {
            self.pending = Some(Pending::Switch(idx));
        } else {
            self.open(idx);
        }
    }

    /// Render the unsaved-changes dialog when an action is pending, and act on
    /// the user's choice.
    fn show_guard(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.pending else { return };
        let mut decision: Option<Decision> = None;

        egui::Window::new("Unsaved changes")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label("This request has unsaved edits.");
                ui.add_space(8.0);
                let (save_label, discard_label) = match pending {
                    Pending::Switch(_) => ("Save & switch", "Discard"),
                    Pending::Quit => ("Save & quit", "Discard & quit"),
                };
                ui.horizontal(|ui| {
                    if ui.button(save_label).clicked() {
                        decision = Some(Decision::Save);
                    }
                    if ui.button(discard_label).clicked() {
                        decision = Some(Decision::Discard);
                    }
                    if ui.button("Cancel").clicked() {
                        decision = Some(Decision::Cancel);
                    }
                });
            });

        let Some(decision) = decision else { return };
        self.pending = None;
        match (decision, pending) {
            (Decision::Cancel, _) => {}
            (Decision::Save, Pending::Switch(idx)) => {
                self.save();
                self.open(idx);
            }
            (Decision::Discard, Pending::Switch(idx)) => self.open(idx),
            (Decision::Save, Pending::Quit) => {
                self.save();
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            (Decision::Discard, Pending::Quit) => {
                self.dirty = false;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    fn run(&mut self, ctx: &egui::Context) {
        let path = PathBuf::from(&self.path);
        let items = match format::collect_requests(&path) {
            Ok(items) => items,
            Err(e) => {
                self.status = e.to_string();
                return;
            }
        };
        if items.is_empty() {
            self.status = "no requests found".into();
            return;
        }
        let config = format::find_config(&path);
        let env_vars = if self.env.is_empty() {
            VarMap::new()
        } else {
            match format::find_environment(&path, &self.env) {
                Some(v) => v,
                None => {
                    self.status = format!("environment `{}` not found", self.env);
                    return;
                }
            }
        };
        let mut scope = Scope::layered(&config.variables, &env_vars, &VarMap::new());

        let (tx, rx) = channel();
        self.rx = Some(rx);
        self.running = true;
        self.results.clear();
        self.status = "running…".into();

        let ctx = ctx.clone();
        self.rt.spawn(async move {
            let runner = Runner::new();
            let results = runner.run_all(&items, &mut scope, &RunOptions::default()).await;
            let _ = tx.send(results);
            ctx.request_repaint();
        });
    }

    fn poll(&mut self) {
        let done = self.rx.as_ref().and_then(|rx| rx.try_recv().ok());
        if let Some(results) = done {
            let (ok, failed, errored) = tally(&results);
            self.results = results;
            self.running = false;
            self.rx = None;
            self.status = format!("{ok} passed, {failed} failed, {errored} errored");
        }
    }
}

fn tally(results: &[ExecutionResult]) -> (usize, usize, usize) {
    let mut ok = 0;
    let mut failed = 0;
    let mut errored = 0;
    for r in results {
        match r.status {
            ExecStatus::Ok => ok += 1,
            ExecStatus::Failed => failed += 1,
            ExecStatus::Error => errored += 1,
        }
    }
    (ok, failed, errored)
}

fn status_color(status: ExecStatus) -> egui::Color32 {
    match status {
        ExecStatus::Ok => egui::Color32::from_rgb(78, 201, 163),
        ExecStatus::Failed => egui::Color32::from_rgb(224, 108, 117),
        ExecStatus::Error => egui::Color32::from_rgb(217, 164, 65),
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll();
        self.poll_update();

        // Ctrl/Cmd+S saves the current request.
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::S)) && self.dirty {
            self.save();
        }

        // Guard the window close (X) against unsaved edits.
        if self.dirty
            && self.pending.is_none()
            && ctx.input(|i| i.viewport().close_requested())
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.pending = Some(Pending::Quit);
        }

        egui::TopBottomPanel::top("bar").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.heading("protoglot");
            ui.horizontal(|ui| {
                ui.label("Collection:");
                ui.add(egui::TextEdit::singleline(&mut self.path).desired_width(360.0));
                if ui.button("📁").on_hover_text("Pick folder").clicked() {
                    if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                        self.path = dir.display().to_string();
                    }
                }
                ui.label("Env:");
                ui.add(egui::TextEdit::singleline(&mut self.env).desired_width(80.0));
                if ui.button("Load").clicked() {
                    self.load();
                }
                let can_run = !self.running && !self.requests.is_empty();
                let label = if self.running { "Running…" } else { "Run all" };
                if ui
                    .add_enabled(can_run, egui::Button::new(label))
                    .clicked()
                {
                    self.run(ctx);
                }
                let can_save = self.selected.is_some() && self.dirty;
                if ui
                    .add_enabled(can_save, egui::Button::new("Save"))
                    .clicked()
                {
                    self.save();
                }
                ui.separator();
                if ui
                    .add_enabled(!self.update_busy, egui::Button::new("Check updates"))
                    .clicked()
                {
                    self.check_updates(ctx);
                }
                if let Some(v) = self.update_offer.clone() {
                    if ui
                        .add_enabled(!self.update_busy, egui::Button::new(format!("Install v{v}")))
                        .clicked()
                    {
                        self.install_update(ctx);
                    }
                }
            });
            if !self.status.is_empty() {
                ui.label(&self.status);
            }
            if !self.update_msg.is_empty() {
                ui.weak(&self.update_msg);
            }
            ui.add_space(4.0);
        });

        egui::SidePanel::left("requests")
            .resizable(true)
            .default_width(280.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    if self.requests.is_empty() {
                        ui.weak("Load a collection to see its requests.");
                    }
                    for idx in 0..self.requests.len() {
                        let name = self.requests[idx].name.clone();
                        let kind = self.requests[idx].kind.clone();
                        let color = self
                            .results
                            .iter()
                            .find(|r| r.request_name == name)
                            .map(|r| status_color(r.status))
                            .unwrap_or(egui::Color32::DARK_GRAY);
                        let selected = self.selected == Some(idx);
                        let mut clicked = false;
                        ui.horizontal(|ui| {
                            ui.colored_label(color, "●");
                            if ui.selectable_label(selected, &name).clicked() {
                                clicked = true;
                            }
                            ui.weak(&kind);
                        });
                        if clicked {
                            self.request_open(idx);
                        }
                    }
                });
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                if self.selected.is_some() {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("SOURCE").weak());
                        if self.dirty {
                            ui.weak("• edited (Ctrl+S not bound; use Save)");
                        }
                    });
                    let mut layouter =
                        |ui: &egui::Ui, text: &str, wrap_width: f32| {
                            let mut job = highlight::toml_highlight(text);
                            job.wrap.max_width = wrap_width;
                            ui.fonts(|f| f.layout_job(job))
                        };
                    let resp = ui.add(
                        egui::TextEdit::multiline(&mut self.source)
                            .desired_width(f32::INFINITY)
                            .desired_rows(18)
                            .layouter(&mut layouter),
                    );
                    if resp.changed() {
                        self.dirty = true;
                    }
                    ui.add_space(12.0);
                }

                if !self.results.is_empty() {
                    ui.label(egui::RichText::new("RESULTS").strong().weak());
                    for r in &self.results {
                        ui.group(|ui| {
                            ui.horizontal(|ui| {
                                ui.colored_label(status_color(r.status), "●");
                                ui.strong(&r.request_name);
                                ui.weak(format!("{:?}", r.protocol).to_lowercase());
                                if let Some(resp) = &r.response {
                                    ui.weak(resp.status.to_string());
                                }
                                ui.weak(format!("{}ms", r.duration.as_millis()));
                            });
                            if let Some(err) = &r.error {
                                ui.colored_label(status_color(ExecStatus::Error), err);
                            }
                            for a in &r.assertions {
                                let mark = if a.passed { "✓" } else { "✗" };
                                let color = if a.passed {
                                    status_color(ExecStatus::Ok)
                                } else {
                                    status_color(ExecStatus::Failed)
                                };
                                let text = match &a.message {
                                    Some(m) => format!("{mark} {} — {m}", a.description),
                                    None => format!("{mark} {}", a.description),
                                };
                                ui.colored_label(color, text);
                            }
                        });
                    }
                }
            });
        });

        self.show_guard(ctx);
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 740.0])
            .with_title("protoglot"),
        ..Default::default()
    };
    eframe::run_native(
        "protoglot",
        options,
        Box::new(|_cc| Ok(Box::new(App::new()))),
    )
}
