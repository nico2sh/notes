use std::{
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};

use crossbeam_channel::{Receiver, Sender};
use eframe::egui;
use kimun_core::{nfs::VaultPath, NoteVault};
use log::{debug, error};

use crate::editor::NoteViewer;

use super::{highlighter::MemoizedNoteHighlighter, EditorMessage, ID_VIEWER};

const UPDATE_TITLE_EVERY_MS: u64 = 500;

pub struct EditorView {
    highlighter: MemoizedNoteHighlighter,
    title: Arc<Mutex<String>>,
    path: VaultPath,
    title_update: Sender<String>,
    last_title_update: SystemTime,
    pending_title_update: bool,
}

impl EditorView {
    pub(super) fn new(path: &VaultPath) -> Self {
        let highlighter = MemoizedNoteHighlighter::default();
        let title = Arc::new(Mutex::new(String::new()));
        let (title_update, receiver) = crossbeam_channel::unbounded::<String>();
        let editor_view = Self {
            highlighter,
            title,
            path: path.to_owned(),
            title_update,
            last_title_update: SystemTime::UNIX_EPOCH,
            pending_title_update: true,
        };
        editor_view.title_update_loop(receiver);
        editor_view
    }

    fn title_update_loop(&self, receiver: Receiver<String>) {
        let title_to_update = self.title.clone();
        std::thread::spawn(move || {
            while let Ok(text) = receiver.recv() {
                let title = NoteVault::get_title(text);
                *title_to_update.lock().unwrap() =
                    title.unwrap_or_else(|| "<Untitled>".to_string());
            }
        });
    }
}

impl NoteViewer for EditorView {
    fn view(&mut self, text: &mut String, ui: &mut eframe::egui::Ui) -> anyhow::Result<bool> {
        let mut layouter = |ui: &egui::Ui, easymark: &str, wrap_width: f32| {
            let mut layout_job = self.highlighter.highlight(ui.style(), easymark);
            layout_job.wrap.max_width = wrap_width;
            ui.fonts(|f| f.layout_job(layout_job))
        };

        let title = self.title.lock().unwrap().clone();
        egui::TopBottomPanel::top("title")
            .resizable(false)
            .min_height(48.0)
            .show_inside(ui, |ui| {
                ui.vertical(|ui| {
                    ui.heading(title);
                    ui.label(self.path.to_string());
                })
            });
        let output = egui::TextEdit::multiline(text)
            .font(egui::TextStyle::Monospace) // for cursor height
            .code_editor()
            .lock_focus(true)
            .cursor_at_end(true)
            .desired_width(f32::INFINITY)
            .layouter(&mut layouter)
            .id(ID_VIEWER.into());
        let response = ui.add_sized(ui.available_size(), output);

        let text_edit_id = response.id;
        if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), text_edit_id) {
            if let Some(range) = state.cursor.char_range() {};
        };
        let changed = if response.changed() {
            self.pending_title_update = true;
            true
        } else {
            false
        };
        if self.pending_title_update
            && SystemTime::now()
                .duration_since(self.last_title_update)
                .map_or_else(
                    |_e| true,
                    |d| d >= Duration::from_millis(UPDATE_TITLE_EVERY_MS),
                )
        {
            debug!("Sending a title update message");
            if let Err(e) = self.title_update.send(text.clone()) {
                error!("Error sending an update to the title: {}", e);
            } else {
                self.last_title_update = SystemTime::now();
                self.pending_title_update = false;
            }
        }
        Ok(changed)
    }

    fn manage_keys(&mut self, ctx: &egui::Context) -> Option<EditorMessage> {
        if ctx.input_mut(|input| {
            input.consume_key(
                egui::Modifiers {
                    command: true,
                    shift: true,
                    ..Default::default()
                },
                egui::Key::Space,
            )
        }) {
            Some(EditorMessage::SwitchNoteViewer(super::ViewerType::Preview(
                self.path.clone(),
            )))
        } else {
            None
        }
    }

    fn init(&mut self, text: String) {
        if let Err(e) = self.title_update.send(text) {
            error!("Error sending an init message for setting the title: {}", e);
        }
    }

    fn view_change_on_content(&self, vault_path: &VaultPath) -> Box<dyn NoteViewer> {
        Box::new(EditorView::new(vault_path))
    }
}
