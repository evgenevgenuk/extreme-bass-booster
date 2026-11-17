#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod batch;
mod dsp;
mod player;

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

use eframe::egui::{self, Align, Color32, Layout, RichText, Vec2};
use rfd::{FileDialog, MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};

use audio::{AudioData, SUPPORTED_EXTENSIONS};
use dsp::BoostMode;
use player::Player;

const BG: Color32 = Color32::from_rgb(30, 30, 30);
const ACCENT: Color32 = Color32::from_rgb(0, 200, 255);

enum Msg {
    Progress(String),
    Loaded(PathBuf, Arc<AudioData>),
    Boosted(Arc<AudioData>),
    Done(String),
    Failed(String),
}

struct App {
    source_path: Option<PathBuf>,
    source: Option<Arc<AudioData>>,
    boost: u32,
    status: String,
    task: Option<Receiver<Msg>>,
    player: Option<Player>,
    seek_pos: f64,
    seek_dragging: bool,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = BG;
        visuals.window_fill = BG;
        cc.egui_ctx.set_visuals(visuals);

        let (player, status) = match Player::new() {
            Ok(p) => (Some(p), "Ready. Load an audio file to start.".to_string()),
            Err(e) => (None, format!("Playback disabled: {e}")),
        };

        Self {
            source_path: None,
            source: None,
            boost: 100,
            status,
            task: None,
            player,
            seek_pos: 0.0,
            seek_dragging: false,
        }
    }

    fn busy(&self) -> bool {
        self.task.is_some()
    }

    fn mode(&self, hyper: bool) -> BoostMode {
        if hyper {
            BoostMode::Hyper
        } else {
            BoostMode::Slider(self.boost)
        }
    }

    /// Runs `job` on a worker thread; its messages are picked up in `update`.
    fn spawn(&mut self, status: &str, job: impl FnOnce(Sender<Msg>) + Send + 'static) {
        let (tx, rx) = channel();
        self.status = status.to_string();
        self.task = Some(rx);
        std::thread::spawn(move || job(tx));
    }

    fn confirm_loud(mode: BoostMode) -> bool {
        if !mode.is_dangerous() {
            return true;
        }
        MessageDialog::new()
            .set_level(MessageLevel::Warning)
            .set_title("Warning")
            .set_description(
                "VERY LOUD!\n\nThis setting can damage your speakers or headphones. \
                 Turn your volume down before listening.\n\nContinue?",
            )
            .set_buttons(MessageButtons::YesNo)
            .show()
            == MessageDialogResult::Yes
    }

    fn audio_dialog() -> FileDialog {
        FileDialog::new()
            .add_filter("Audio files", SUPPORTED_EXTENSIONS)
            .add_filter("All files", &["*"])
    }

    fn pick_file(&mut self) {
        let Some(path) = Self::audio_dialog().pick_file() else { return };
        self.stop();
        self.source = None;
        self.source_path = None;
        self.spawn("Decoding...", move |tx| {
            let msg = match audio::decode_file(&path) {
                Ok(a) => Msg::Loaded(path, Arc::new(a)),
                Err(e) => Msg::Failed(format!("Could not load file: {e:#}")),
            };
            let _ = tx.send(msg);
        });
    }

    fn require_source(&mut self) -> Option<Arc<AudioData>> {
        if self.source.is_none() {
            MessageDialog::new()
                .set_title("No file")
                .set_description("Load an audio file first.")
                .set_buttons(MessageButtons::Ok)
                .show();
        }
        self.source.clone()
    }

    fn play(&mut self, hyper: bool) {
        let Some(src) = self.require_source() else { return };
        let mode = self.mode(hyper);
        if !Self::confirm_loud(mode) {
            return;
        }
        self.stop();
        self.spawn("Boosting...", move |tx| {
            let _ = tx.send(Msg::Boosted(Arc::new(dsp::boost(&src, mode))));
        });
    }

    fn stop(&mut self) {
        if let Some(p) = self.player.as_mut() {
            p.stop();
        }
        self.seek_pos = 0.0;
    }

    fn save(&mut self, hyper: bool) {
        let Some(src) = self.require_source() else { return };
        let mode = self.mode(hyper);
        if !Self::confirm_loud(mode) {
            return;
        }
        let stem = self
            .source_path
            .as_ref()
            .and_then(|p| p.file_stem())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "output".into());
        let Some(mut out) = FileDialog::new()
            .add_filter("WAV audio", &["wav"])
            .set_file_name(format!("{}{stem}.wav", mode.file_prefix()))
            .save_file()
        else {
            return;
        };
        if out.extension().is_none() {
            out.set_extension("wav");
        }
        self.spawn("Boosting and saving...", move |tx| {
            let msg = match audio::save_wav(&dsp::boost(&src, mode), &out) {
                Ok(()) => Msg::Done(format!("Saved: {}", out.display())),
                Err(e) => Msg::Failed(format!("Save failed: {e:#}")),
            };
            let _ = tx.send(msg);
        });
    }

    fn process_zip(&mut self, hyper: bool) {
        let mode = self.mode(hyper);
        if !Self::confirm_loud(mode) {
            return;
        }
        let Some(input) = FileDialog::new().add_filter("ZIP archive", &["zip"]).pick_file() else {
            return;
        };
        let default_name = if hyper { "hyper_all.zip" } else { "boosted_all.zip" };
        let Some(mut output) = FileDialog::new()
            .add_filter("ZIP archive", &["zip"])
            .set_file_name(default_name)
            .save_file()
        else {
            return;
        };
        if !output.to_string_lossy().to_ascii_lowercase().ends_with(".zip") {
            output.as_mut_os_string().push(".zip");
        }
        if output == input {
            self.status = "Output ZIP must be different from the input ZIP.".into();
            return;
        }

        self.spawn("Processing ZIP...", move |tx| {
            let progress_tx = tx.clone();
            let result = batch::process_zip(&input, &output, mode, |done, total, name| {
                if !name.is_empty() {
                    let _ = progress_tx.send(Msg::Progress(format!("[{}/{total}] {name}", done + 1)));
                }
            });
            let msg = match result {
                Ok(r) => {
                    let mut text = format!(
                        "ZIP done: {} boosted, {} copied as-is -> {}",
                        r.boosted,
                        r.copied,
                        output.display()
                    );
                    if !r.failed.is_empty() {
                        text.push_str(&format!("\nCould not decode {} file(s):", r.failed.len()));
                        for f in r.failed.iter().take(5) {
                            text.push_str(&format!("\n  {f}"));
                        }
                    }
                    Msg::Done(text)
                }
                Err(e) => Msg::Failed(format!("ZIP failed: {e:#}")),
            };
            let _ = tx.send(msg);
        });
    }

    fn poll_task(&mut self) {
        let Some(rx) = self.task.as_ref() else { return };
        let mut finished = false;
        while let Ok(msg) = rx.try_recv() {
            match msg {
                Msg::Progress(s) => self.status = s,
                Msg::Loaded(path, audio) => {
                    self.status = format!(
                        "Loaded: {:.1} s, {} Hz, {} ch",
                        audio.duration_secs(),
                        audio.sample_rate,
                        audio.channels
                    );
                    self.source_path = Some(path);
                    self.source = Some(audio);
                    finished = true;
                }
                Msg::Boosted(audio) => {
                    self.status = match self.player.as_mut() {
                        Some(p) => match p.play(audio) {
                            Ok(()) => "Playing".into(),
                            Err(e) => format!("Playback error: {e:#}"),
                        },
                        None => "No audio output device available.".into(),
                    };
                    finished = true;
                }
                Msg::Done(s) | Msg::Failed(s) => {
                    self.status = s;
                    finished = true;
                }
            }
        }
        if finished {
            self.task = None;
        }
    }
}

fn big_button(ui: &mut egui::Ui, text: &str, color: Color32, size: Vec2, enabled: bool) -> bool {
    let button = egui::Button::new(RichText::new(text).size(16.0).color(Color32::WHITE).strong())
        .fill(color)
        .min_size(size);
    ui.add_enabled(enabled, button).clicked()
}

fn fmt_time(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_task();
        let idle = !self.busy();
        let has_file = self.source.is_some();

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.with_layout(Layout::top_down(Align::Center), |ui| {
                ui.add_space(16.0);
                ui.label(RichText::new("Extreme Bass Booster 9000").size(28.0).strong().color(ACCENT));
                ui.add_space(8.0);

                let name = self
                    .source_path
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "none".into());
                ui.label(RichText::new(format!("File: {name}")).size(16.0).color(Color32::WHITE));
                ui.add_space(8.0);

                let wide = Vec2::new(400.0, 46.0);
                if big_button(ui, "Load audio file", Color32::from_rgb(0, 120, 215), wide, idle) {
                    self.pick_file();
                }
                if big_button(ui, "Boost ZIP (slider)", Color32::from_rgb(200, 0, 100), wide, idle) {
                    self.process_zip(false);
                }
                if big_button(ui, "HYPER MAX for ZIP", Color32::from_rgb(220, 0, 0), wide, idle) {
                    self.process_zip(true);
                }

                ui.add_space(14.0);
                ui.spacing_mut().slider_width = 480.0;
                ui.add(egui::Slider::new(&mut self.boost, 0..=400).show_value(false));
                ui.label(
                    RichText::new(format!("{} %", self.boost))
                        .size(30.0)
                        .strong()
                        .color(Color32::from_rgb(0, 255, 100)),
                );
                ui.add_space(10.0);

                let half = Vec2::new(240.0, 54.0);
                ui.horizontal(|ui| {
                    ui.add_space((ui.available_width() - 2.0 * half.x - ui.spacing().item_spacing.x) / 2.0);
                    if big_button(ui, "Play (slider)", Color32::from_rgb(0, 160, 0), half, idle && has_file) {
                        self.play(false);
                    }
                    if big_button(ui, "Play HYPER MAX", Color32::from_rgb(220, 0, 0), half, idle && has_file) {
                        self.play(true);
                    }
                });

                let playing = self.player.as_ref().is_some_and(|p| p.is_playing());
                if big_button(ui, "Stop", Color32::from_rgb(150, 0, 0), Vec2::new(110.0, 42.0), playing) {
                    self.stop();
                }

                // Seek bar
                ui.add_space(8.0);
                let duration = self.player.as_ref().map_or(0.0, |p| p.duration());
                if !self.seek_dragging {
                    self.seek_pos = self.player.as_ref().map_or(0.0, |p| p.position()).min(duration);
                }
                let resp = ui.add_enabled(
                    playing,
                    egui::Slider::new(&mut self.seek_pos, 0.0..=duration.max(0.001)).show_value(false),
                );
                self.seek_dragging = resp.dragged();
                if playing && (resp.drag_stopped() || (resp.changed() && !resp.dragged())) {
                    if let Some(p) = self.player.as_mut() {
                        if let Err(e) = p.seek(self.seek_pos) {
                            self.status = format!("Seek error: {e:#}");
                        }
                    }
                }
                ui.label(
                    RichText::new(format!("{} / {}", fmt_time(self.seek_pos), fmt_time(duration)))
                        .color(Color32::LIGHT_GRAY),
                );

                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.add_space((ui.available_width() - 2.0 * half.x - ui.spacing().item_spacing.x) / 2.0);
                    if big_button(ui, "Save (slider)", Color32::from_rgb(255, 140, 0), half, idle && has_file) {
                        self.save(false);
                    }
                    if big_button(ui, "Save HYPER MAX", Color32::from_rgb(200, 0, 200), half, idle && has_file) {
                        self.save(true);
                    }
                });

                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    if !idle {
                        ui.spinner();
                    }
                    ui.label(RichText::new(&self.status).color(Color32::LIGHT_GRAY));
                });
            });
        });

        if self.busy() || self.player.as_ref().is_some_and(|p| p.is_playing()) {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Extreme Bass Booster 9000")
            .with_inner_size([600.0, 720.0])
            .with_min_inner_size([560.0, 680.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Extreme Bass Booster 9000",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}
