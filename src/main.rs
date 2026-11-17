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
use dsp::{BoostMode, Equalizer, EQ_BANDS, EQ_RANGE_DB};
use player::Player;

const BG: Color32 = Color32::from_rgb(30, 30, 30);
const ACCENT: Color32 = Color32::from_rgb(0, 200, 255);

enum Msg {
    Progress(String),
    Loaded(PathBuf, Arc<AudioData>),
    /// Processed audio ready to play, and the position to start from.
    Boosted(Arc<AudioData>, f64),
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
    eq: Equalizer,
    /// Settings of the track currently playing: (hyper, mode, eq).
    /// Used to re-render live when the slider or equalizer changes.
    playing_with: Option<(bool, BoostMode, Equalizer)>,
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
            eq: Equalizer::default(),
            playing_with: None,
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
        let Some(path) = Self::audio_dialog().pick_file() else {
            return;
        };
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
        let Some(src) = self.require_source() else {
            return;
        };
        let mode = self.mode(hyper);
        if !Self::confirm_loud(mode) {
            return;
        }
        self.stop();
        self.render_and_play(src, hyper, 0.0, "Boosting...");
    }

    fn render_and_play(&mut self, src: Arc<AudioData>, hyper: bool, from: f64, status: &str) {
        let mode = self.mode(hyper);
        let eq = self.eq;
        self.playing_with = Some((hyper, mode, eq));
        self.spawn(status, move |tx| {
            let _ = tx.send(Msg::Boosted(Arc::new(dsp::process(&src, mode, &eq)), from));
        });
    }

    /// If the slider or equalizer changed while playing, re-render and
    /// continue from the same position.
    fn rerender_if_changed(&mut self, pointer_down: bool) {
        let Some((hyper, mode, eq)) = self.playing_with else {
            return;
        };
        if pointer_down || self.busy() || (self.mode(hyper) == mode && self.eq == eq) {
            return;
        }
        let playing = self.player.as_ref().is_some_and(|p| p.is_playing());
        let Some(src) = self.source.clone().filter(|_| playing) else {
            self.playing_with = None;
            return;
        };
        let pos = self.player.as_ref().map_or(0.0, |p| p.position());
        self.render_and_play(src, hyper, pos, "Applying changes...");
    }

    fn stop(&mut self) {
        if let Some(p) = self.player.as_mut() {
            p.stop();
        }
        self.playing_with = None;
        self.seek_pos = 0.0;
    }

    fn equalizer_ui(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Equalizer").size(18.0).strong().color(ACCENT));
        ui.add_space(4.0);

        ui.horizontal(|ui| {
            let width = 180.0 + 80.0 + ui.spacing().item_spacing.x;
            ui.add_space((ui.available_width() - width) / 2.0);
            let current = self.eq.preset_name().unwrap_or("Custom");
            egui::ComboBox::from_id_salt("eq_preset")
                .selected_text(current)
                .width(180.0)
                .show_ui(ui, |ui| {
                    for (name, gains) in Equalizer::PRESETS {
                        if ui.selectable_label(current == *name, *name).clicked() {
                            self.eq.gains = *gains;
                        }
                    }
                });
            if ui
                .add_sized([80.0, 20.0], egui::Button::new("Reset"))
                .clicked()
            {
                self.eq = Equalizer::default();
            }
        });
        ui.add_space(6.0);

        let col = 46.0;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.spacing_mut().slider_width = 130.0;
            let total = col * EQ_BANDS.len() as f32 + 4.0 * (EQ_BANDS.len() - 1) as f32;
            ui.add_space(((ui.available_width() - total) / 2.0).max(0.0));
            for (i, freq) in EQ_BANDS.iter().enumerate() {
                ui.allocate_ui_with_layout(
                    Vec2::new(col, 190.0),
                    Layout::top_down(Align::Center),
                    |ui| {
                        let g = self.eq.gains[i];
                        let color = if g > 0.05 {
                            Color32::from_rgb(0, 255, 100)
                        } else if g < -0.05 {
                            Color32::from_rgb(255, 120, 80)
                        } else {
                            Color32::LIGHT_GRAY
                        };
                        ui.label(RichText::new(format!("{g:+.0}")).size(13.0).color(color));
                        let resp = ui.add(
                            egui::Slider::new(&mut self.eq.gains[i], -EQ_RANGE_DB..=EQ_RANGE_DB)
                                .vertical()
                                .step_by(0.5)
                                .show_value(false),
                        );
                        if resp.double_clicked() {
                            self.eq.gains[i] = 0.0;
                        }
                        let label = if *freq >= 1000.0 {
                            format!("{}k", freq / 1000.0)
                        } else {
                            format!("{freq}")
                        };
                        ui.label(RichText::new(label).size(12.0).color(Color32::GRAY));
                    },
                );
            }
        });
        ui.label(
            RichText::new(
                "dB per band. Double-click a slider to reset it. Changes apply live while playing.",
            )
            .size(11.0)
            .color(Color32::GRAY),
        );
    }

    fn save(&mut self, hyper: bool) {
        let Some(src) = self.require_source() else {
            return;
        };
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
        let eq = self.eq;
        self.spawn("Boosting and saving...", move |tx| {
            let msg = match audio::save_wav(&dsp::process(&src, mode, &eq), &out) {
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
        let Some(input) = FileDialog::new()
            .add_filter("ZIP archive", &["zip"])
            .pick_file()
        else {
            return;
        };
        let default_name = if hyper {
            "hyper_all.zip"
        } else {
            "boosted_all.zip"
        };
        let Some(mut output) = FileDialog::new()
            .add_filter("ZIP archive", &["zip"])
            .set_file_name(default_name)
            .save_file()
        else {
            return;
        };
        if !output
            .to_string_lossy()
            .to_ascii_lowercase()
            .ends_with(".zip")
        {
            output.as_mut_os_string().push(".zip");
        }
        if output == input {
            self.status = "Output ZIP must be different from the input ZIP.".into();
            return;
        }

        let eq = self.eq;
        self.spawn("Processing ZIP...", move |tx| {
            let progress_tx = tx.clone();
            let result = batch::process_zip(&input, &output, mode, eq, |done, total, name| {
                if !name.is_empty() {
                    let _ =
                        progress_tx.send(Msg::Progress(format!("[{}/{total}] {name}", done + 1)));
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
                Msg::Boosted(audio, from) => {
                    self.status = match self.player.as_mut() {
                        Some(p) => match p.play_at(audio, from) {
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
    let button = egui::Button::new(
        RichText::new(text)
            .size(16.0)
            .color(Color32::WHITE)
            .strong(),
    )
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
        self.rerender_if_changed(ctx.input(|i| i.pointer.any_down()));
        let idle = !self.busy();
        let has_file = self.source.is_some();

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.with_layout(Layout::top_down(Align::Center), |ui| {
                        ui.add_space(16.0);
                        ui.label(
                            RichText::new("Extreme Bass Booster 9000")
                                .size(28.0)
                                .strong()
                                .color(ACCENT),
                        );
                        ui.add_space(8.0);

                        let name = self
                            .source_path
                            .as_ref()
                            .and_then(|p| p.file_name())
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "none".into());
                        ui.label(
                            RichText::new(format!("File: {name}"))
                                .size(16.0)
                                .color(Color32::WHITE),
                        );
                        ui.add_space(8.0);

                        let wide = Vec2::new(400.0, 46.0);
                        if big_button(
                            ui,
                            "Load audio file",
                            Color32::from_rgb(0, 120, 215),
                            wide,
                            idle,
                        ) {
                            self.pick_file();
                        }
                        if big_button(
                            ui,
                            "Boost ZIP (slider)",
                            Color32::from_rgb(200, 0, 100),
                            wide,
                            idle,
                        ) {
                            self.process_zip(false);
                        }
                        if big_button(
                            ui,
                            "HYPER MAX for ZIP",
                            Color32::from_rgb(220, 0, 0),
                            wide,
                            idle,
                        ) {
                            self.process_zip(true);
                        }

                        ui.add_space(14.0);
                        ui.spacing_mut().slider_width = 480.0;
                        ui.add_sized(
                            [480.0, 20.0],
                            egui::Slider::new(&mut self.boost, 0..=400).show_value(false),
                        );
                        ui.label(
                            RichText::new(format!("{} %", self.boost))
                                .size(30.0)
                                .strong()
                                .color(Color32::from_rgb(0, 255, 100)),
                        );
                        ui.add_space(10.0);
                        ui.separator();
                        self.equalizer_ui(ui);
                        ui.separator();
                        ui.add_space(10.0);

                        let half = Vec2::new(240.0, 54.0);
                        ui.horizontal(|ui| {
                            ui.add_space(
                                (ui.available_width() - 2.0 * half.x - ui.spacing().item_spacing.x)
                                    / 2.0,
                            );
                            if big_button(
                                ui,
                                "Play (slider)",
                                Color32::from_rgb(0, 160, 0),
                                half,
                                idle && has_file,
                            ) {
                                self.play(false);
                            }
                            if big_button(
                                ui,
                                "Play HYPER MAX",
                                Color32::from_rgb(220, 0, 0),
                                half,
                                idle && has_file,
                            ) {
                                self.play(true);
                            }
                        });

                        let playing = self.player.as_ref().is_some_and(|p| p.is_playing());
                        if big_button(
                            ui,
                            "Stop",
                            Color32::from_rgb(150, 0, 0),
                            Vec2::new(110.0, 42.0),
                            playing,
                        ) {
                            self.stop();
                        }

                        // Seek bar
                        ui.add_space(8.0);
                        let duration = self.player.as_ref().map_or(0.0, |p| p.duration());
                        if !self.seek_dragging {
                            self.seek_pos = self
                                .player
                                .as_ref()
                                .map_or(0.0, |p| p.position())
                                .min(duration);
                        }
                        ui.spacing_mut().slider_width = 480.0;
                        let resp = ui
                            .add_enabled_ui(playing, |ui| {
                                ui.add_sized(
                                    [480.0, 20.0],
                                    egui::Slider::new(
                                        &mut self.seek_pos,
                                        0.0..=duration.max(0.001),
                                    )
                                    .show_value(false),
                                )
                            })
                            .inner;
                        self.seek_dragging = resp.dragged();
                        if playing && (resp.drag_stopped() || (resp.changed() && !resp.dragged())) {
                            if let Some(p) = self.player.as_mut() {
                                if let Err(e) = p.seek(self.seek_pos) {
                                    self.status = format!("Seek error: {e:#}");
                                }
                            }
                        }
                        ui.label(
                            RichText::new(format!(
                                "{} / {}",
                                fmt_time(self.seek_pos),
                                fmt_time(duration)
                            ))
                            .color(Color32::LIGHT_GRAY),
                        );

                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            ui.add_space(
                                (ui.available_width() - 2.0 * half.x - ui.spacing().item_spacing.x)
                                    / 2.0,
                            );
                            if big_button(
                                ui,
                                "Save (slider)",
                                Color32::from_rgb(255, 140, 0),
                                half,
                                idle && has_file,
                            ) {
                                self.save(false);
                            }
                            if big_button(
                                ui,
                                "Save HYPER MAX",
                                Color32::from_rgb(200, 0, 200),
                                half,
                                idle && has_file,
                            ) {
                                self.save(true);
                            }
                        });

                        ui.add_space(14.0);
                        if !idle {
                            ui.spinner();
                        }
                        ui.label(RichText::new(&self.status).color(Color32::LIGHT_GRAY));
                        ui.add_space(10.0);
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
            .with_inner_size([620.0, 900.0])
            .with_min_inner_size([580.0, 500.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Extreme Bass Booster 9000",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}
