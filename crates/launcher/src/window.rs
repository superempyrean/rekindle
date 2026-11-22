//! The setup window: draws a [`Model`] with egui and runs its jobs on a
//! thread, so the window stays responsive while the client starts.
//!
//! [`draw`] is separate from the eframe app so the tests can run it in a
//! headless egui context, pages and button clicks included.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};

use eframe::egui;
use egui::{Align, Color32, CornerRadius, Layout, RichText, Vec2};

use crate::ui::{Action, CLIENT, Job, Model, Outcome, Page};

/// What the window does for real (`main` implements it).
pub trait Backend: Send + 'static {
    /// Run a job: install with `client`, start the client, or uninstall.
    fn run(&mut self, job: &Job, client: Option<PathBuf>) -> Outcome;
}

const BG: Color32 = Color32::from_rgb(0x31, 0x33, 0x38);
const TEXT: Color32 = Color32::from_rgb(0xF2, 0xF3, 0xF5);
const MUTED: Color32 = Color32::from_rgb(0xB5, 0xBA, 0xC1);
const BLURPLE: Color32 = Color32::from_rgb(0x58, 0x65, 0xF2);
const RED: Color32 = Color32::from_rgb(0xDA, 0x37, 0x3C);
/// The fill of a button that isn't the page's main one.
const SECONDARY: Color32 = Color32::from_gray(0x4E);

/// Show the window until it's closed. Errors if no window could be made
/// (no OpenGL, for one); the caller then falls back to message boxes.
pub fn run(
    model: Model,
    backend: impl Backend,
    pick: fn() -> Option<PathBuf>,
) -> Result<(), String> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("rekindle")
            .with_inner_size([460.0, 340.0])
            .with_resizable(false)
            .with_maximize_button(false),
        centered: true,
        ..Default::default()
    };
    let app = App {
        model,
        backend: Some(Box::new(backend)),
        running: None,
        pick,
    };
    eframe::run_native(
        "rekindle",
        options,
        Box::new(|cc| {
            style(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| e.to_string())
}

/// A job's outcome, and the backend coming back from its thread.
type Finished = (Outcome, Box<dyn Backend>);

struct App {
    model: Model,
    /// The backend, while no job has it.
    backend: Option<Box<dyn Backend>>,
    running: Option<(Job, Receiver<Finished>)>,
    pick: fn() -> Option<PathBuf>,
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if let Some((job, rx)) = &self.running {
            if let Ok((outcome, backend)) = rx.try_recv() {
                let job = job.clone();
                self.backend = Some(backend);
                self.running = None;
                self.model.finished(&job, outcome);
            } else {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(100));
            }
        }
        if self.model.want_picker {
            let picked = (self.pick)();
            self.model.act(Action::Picked(picked));
        }
        let mut hits = Vec::new();
        if let Some(action) = draw(ui, &self.model, &mut hits)
            && let Some(job) = self.model.act(action)
        {
            self.start(job, ui.ctx().clone());
        }
        if self.model.page == Page::Closed {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

impl App {
    fn start(&mut self, job: Job, ctx: egui::Context) {
        let Some(mut backend) = self.backend.take() else {
            return;
        };
        let (tx, rx) = channel();
        let client = self.model.client.clone();
        let j = job.clone();
        std::thread::spawn(move || {
            let outcome = backend.run(&j, client);
            let _ = tx.send((outcome, backend));
            ctx.request_repaint();
        });
        self.running = Some((job, rx));
    }
}

fn style(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = BG;
    v.override_text_color = Some(TEXT);
    v.hyperlink_color = MUTED;
    ctx.set_visuals(v);
}

/// Draw the page; returns what the user clicked this frame. `hits` gets each
/// button's action and place, for the tests.
pub fn draw(ui: &mut egui::Ui, m: &Model, hits: &mut Vec<(Action, egui::Rect)>) -> Option<Action> {
    let mut clicked = None;
    let frame = egui::Frame::central_panel(ui.style())
        .fill(BG)
        .inner_margin(28.0);
    egui::CentralPanel::default().frame(frame).show(ui, |ui| {
        ui.label(RichText::new("rekindle").size(30.0).strong());
        ui.label(
            RichText::new(format!("Plugins and themes for {CLIENT}"))
                .size(15.0)
                .color(MUTED),
        );
        ui.add_space(22.0);
        page(ui, m, &mut |action: Action, r: &egui::Response| {
            hits.push((action.clone(), r.rect));
            if r.clicked() {
                clicked = Some(action);
            }
        });
    });
    clicked
}

/// The page's own text and buttons. `record` gets each button and the
/// action clicking it takes.
fn page(ui: &mut egui::Ui, m: &Model, record: &mut dyn FnMut(Action, &egui::Response)) {
    let primary = m.primary_label().unwrap_or_default();
    match &m.page {
        Page::Welcome => {
            if m.client.is_none() {
                say(
                    ui,
                    &format!("First, show rekindle where {CLIENT} is installed."),
                );
            } else {
                say(ui, &format!("Ready to add plugins to {CLIENT}."));
            }
            ui.add_space(14.0);
            record(Action::Primary, &button(ui, &primary, BLURPLE, true));
            ui.add_space(14.0);
            small(
                ui,
                &format!(
                    "rekindle installs for you only, next to {CLIENT}, and loads whenever {CLIENT} opens. \
                     Nothing else starts by itself, and you can remove it any time in Settings › Apps."
                ),
            );
        }
        Page::Installed { installed } => {
            say(ui, &format!("rekindle {installed} is already installed."));
            ui.add_space(14.0);
            record(Action::Primary, &button(ui, &primary, BLURPLE, true));
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let open = button(ui, &format!("Open {CLIENT}"), SECONDARY, false);
                record(Action::Open, &open);
                record(
                    Action::StartUninstall,
                    &button(ui, "Uninstall", SECONDARY, false),
                );
            });
        }
        Page::Working(job) => {
            ui.add_space(20.0);
            ui.horizontal(|ui| {
                ui.spinner();
                say(ui, &job.label());
            });
        }
        Page::InstallDone => {
            say(ui, "rekindle is installed.");
            ui.add_space(4.0);
            small(
                ui,
                &format!("From now on, plugins load whenever you open {CLIENT}."),
            );
            ui.add_space(14.0);
            record(Action::Primary, &button(ui, &primary, BLURPLE, true));
            ui.add_space(8.0);
            record(Action::Close, &button(ui, "Close", SECONDARY, false));
        }
        Page::InstalledPlain { details } => {
            say(
                ui,
                &format!("rekindle is installed, but it couldn't start with {CLIENT}."),
            );
            ui.add_space(4.0);
            small(ui, &format!("{CLIENT} will open without plugins."));
            ui.add_space(12.0);
            record(Action::Primary, &button(ui, &primary, BLURPLE, true));
            ui.add_space(8.0);
            record(Action::Close, &button(ui, "Close", SECONDARY, false));
            details_box(ui, details);
        }
        Page::Uninstall { delete_plugins } => {
            say(
                ui,
                &format!("Uninstall rekindle? {CLIENT} itself stays installed."),
            );
            ui.add_space(10.0);
            let mut delete = *delete_plugins;
            let tick = ui.checkbox(&mut delete, "Also delete my plugins and their settings");
            record(Action::ToggleDeletePlugins, &tick);
            ui.add_space(14.0);
            record(Action::Primary, &button(ui, &primary, RED, true));
            ui.add_space(8.0);
            record(Action::Cancel, &button(ui, "Cancel", SECONDARY, false));
        }
        Page::Uninstalled => {
            say(
                ui,
                &format!("rekindle was removed. {CLIENT} is still installed."),
            );
            ui.add_space(14.0);
            record(Action::Primary, &button(ui, &primary, BLURPLE, true));
        }
        Page::Problem { text, details } => {
            say(ui, "Something went wrong.");
            ui.add_space(4.0);
            small(ui, text);
            ui.add_space(12.0);
            record(Action::Primary, &button(ui, &primary, BLURPLE, true));
            details_box(ui, details);
        }
        Page::Closed => {}
    }
}

/// A line of the page's text.
fn say(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(15.0));
}

/// A line of small print.
fn small(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(12.5).color(MUTED));
}

/// One of the page's buttons: big ones fill the width.
fn button(ui: &mut egui::Ui, label: &str, fill: Color32, big: bool) -> egui::Response {
    let text = RichText::new(label)
        .size(if big { 17.0 } else { 14.0 })
        .color(Color32::WHITE);
    let size = if big {
        Vec2::new(ui.available_width(), 44.0)
    } else {
        Vec2::new(0.0, 32.0)
    };
    ui.add(
        egui::Button::new(text)
            .fill(fill)
            .corner_radius(CornerRadius::same(6))
            .min_size(size),
    )
}

/// The technical part, folded away, with a button to copy it for whoever
/// helps.
fn details_box(ui: &mut egui::Ui, details: &str) {
    ui.add_space(8.0);
    egui::CollapsingHeader::new(RichText::new("Details").size(12.5).color(MUTED)).show(ui, |ui| {
        egui::ScrollArea::vertical()
            .max_height(70.0)
            .show(ui, |ui| {
                ui.label(RichText::new(details).size(11.5).monospace().color(MUTED));
            });
        ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
            if ui.small_button("Copy").clicked() {
                ui.ctx().copy_text(details.to_string());
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One frame of `m` in a headless context, with `events` as input.
    fn frame(
        ctx: &egui::Context,
        m: &Model,
        events: Vec<egui::Event>,
    ) -> (Option<Action>, Vec<(Action, egui::Rect)>) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(460.0, 340.0),
            )),
            events,
            ..Default::default()
        };
        let mut out = (None, Vec::new());
        let mut full = ctx.run_ui(input, |ui| out.0 = draw(ui, m, &mut out.1));
        // No renderer here to upload the font texture to.
        full.textures_delta.clear();
        out
    }

    /// Click the button for `action` the way egui sees a click.
    fn click(m: &Model, action: &Action) -> Option<Action> {
        let ctx = egui::Context::default();
        style(&ctx);
        let (_, hits) = frame(&ctx, m, vec![]);
        let rect = hits
            .iter()
            .find(|(a, _)| a == action)
            .unwrap_or_else(|| panic!("no {action:?} button on {:?}", m.page))
            .1;
        assert!(
            rect.max.x <= 460.0 && rect.max.y <= 340.0,
            "{action:?} is outside the window: {rect:?}"
        );
        let pos = rect.center();
        let press = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        frame(&ctx, m, vec![egui::Event::PointerMoved(pos)]);
        frame(&ctx, m, vec![press(true)]);
        frame(&ctx, m, vec![press(false)]).0
    }

    #[test]
    fn every_page_draws_and_its_main_button_clicks() {
        let client = Some(PathBuf::from("C:/Discord/Discord.exe"));
        let pages = [
            Model::setup("0.2.0", None, None),
            Model::setup("0.2.0", None, client.clone()),
            Model::setup("0.3.0", Some("0.2.0".into()), client.clone()),
            Model { page: Page::InstallDone, ..Model::setup("0.2.0", None, client.clone()) },
            Model {
                page: Page::InstalledPlain { details: "the client ended (exit code 0) before rekindle reported (2.1 s); shim: host: loaded".into() },
                ..Model::setup("0.2.0", None, client)
            },
            Model::uninstall(),
            Model { page: Page::Uninstalled, ..Model::uninstall() },
            Model::problem(&anyhow::anyhow!("disk full").context("writing state.json")),
        ];
        for m in &pages {
            assert_eq!(
                click(m, &Action::Primary),
                Some(Action::Primary),
                "{:?}",
                m.page
            );
        }
        assert_eq!(
            click(&pages[2], &Action::StartUninstall),
            Some(Action::StartUninstall)
        );
        assert_eq!(click(&pages[4], &Action::Close), Some(Action::Close));
        assert_eq!(
            click(&pages[5], &Action::ToggleDeletePlugins),
            Some(Action::ToggleDeletePlugins)
        );
        // A running job has no buttons.
        let ctx = egui::Context::default();
        let working = Model {
            page: Page::Working(Job::Launch),
            ..Model::uninstall()
        };
        assert!(frame(&ctx, &working, vec![]).1.is_empty());
    }
}
