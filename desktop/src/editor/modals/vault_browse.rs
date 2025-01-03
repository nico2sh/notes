use std::sync::{
    mpsc::{self, Receiver},
    Arc, Mutex,
};

use eframe::egui;
use log::{debug, error};
use notes_core::{nfs::NotePath, NoteVault, SearchResult, VaultBrowseOptionsBuilder};
use rayon::slice::ParallelSliceMut;

use crate::icons;

use super::{EditorMessage, EditorModal};

pub const ID_SEARCH: &str = "Search Popup";

pub(super) struct VaultBrowse {
    filter_text: String,
    selector: Selector,
    message_sender: mpsc::Sender<EditorMessage>,
    rx: mpsc::Receiver<SearchResult>,
    to_clear: bool,
    requested_focus: bool,
    requested_scroll: bool,
    vault: Arc<NoteVault>,
}

impl VaultBrowse {
    pub fn new(
        vault: NoteVault,
        path: &NotePath,
        message_sender: mpsc::Sender<EditorMessage>,
    ) -> Self {
        let selector = Selector::new();
        let vault = Arc::new(vault);
        let rx = Self::browse_path(vault.clone(), path);

        Self {
            filter_text: String::new(),
            selector,
            message_sender,
            rx,
            to_clear: false,
            requested_focus: true,
            requested_scroll: false,
            vault,
        }
    }

    fn browse_path(vault: Arc<NoteVault>, path: &NotePath) -> Receiver<SearchResult> {
        let search_path = if path.is_note() {
            path.get_parent_path().0
        } else {
            path.to_owned()
        };
        let (browse_options, receiver) = VaultBrowseOptionsBuilder::new(&search_path).build();

        // We fetch the data asynchronously
        std::thread::spawn(move || {
            debug!("Retreiving notes for dialog");
            vault
                .browse_vault(browse_options)
                .expect("Error getting notes");
        });

        receiver
    }

    pub fn clear(&mut self) {
        self.to_clear = true;
    }

    pub fn request_focus(&mut self) {
        self.requested_focus = true;
    }

    fn update_filter(&mut self) {
        self.selector.update_elements();

        let trigger_filter = if let Ok(row) = self.rx.try_recv() {
            // info!("adding to list {}", row.as_ref());
            let mut elements = self.selector.elements.lock().unwrap();
            elements.push(row.into());
            while let Ok(row) = self.rx.recv() {
                elements.push(row.into());
            }
            true
        } else {
            false
        };
        if trigger_filter {
            self.selector.filter_content(&self.filter_text);
        }
    }

    fn open_note(&self, path: &NotePath) {
        if let Err(e) = self
            .message_sender
            .send(EditorMessage::OpenNote(path.clone()))
        {
            error!(
                "Can't send the message to open the note at {}, Err: {}",
                path, e
            )
        };
    }

    fn select(&mut self, selected: &SelectorEntry) {
        match selected.entry_type {
            SelectorEntryType::Note { title: _ } => {
                self.open_note(&selected.path);
            }
            SelectorEntryType::Directory => {
                self.clear();
                self.rx = Self::browse_path(self.vault.clone(), &selected.path);
                self.request_focus();
            }
            SelectorEntryType::Attachment => {}
        }
    }
}

impl EditorModal for VaultBrowse {
    fn update(&mut self, ui: &mut egui::Ui) {
        if self.to_clear {
            self.selector.clear();
            self.to_clear = false;
        }

        self.update_filter();

        ui.with_layout(
            egui::Layout {
                main_dir: egui::Direction::TopDown,
                main_wrap: false,
                main_align: egui::Align::Center,
                main_justify: false,
                cross_align: egui::Align::Min,
                cross_justify: false,
            },
            |ui| {
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.filter_text)
                        .desired_width(f32::INFINITY)
                        .id(ID_SEARCH.into()),
                );

                let mut selected = self.selector.get_selected();
                let scroll_area = egui::scroll_area::ScrollArea::vertical()
                    .max_height(400.0)
                    .auto_shrink(false);
                scroll_area.show(ui, |ui| {
                    ui.vertical(|ui| {
                        // TODO: Avoid cloning the elements
                        for (pos, element) in
                            self.selector.get_elements().clone().iter().enumerate()
                        {
                            let response = element.get_label(ui);
                            if response.clicked() {
                                self.select(element);
                            }
                            if response.hovered() {
                                selected = Some(pos);
                            }
                            if Some(pos) == selected {
                                if self.requested_scroll {
                                    response.scroll_to_me(Some(egui::Align::Center));
                                    self.requested_scroll = false;
                                }
                                response.highlight();
                            }
                        }
                    });
                });
                self.selector.set_selected(selected);

                if response.changed() {
                    self.selector.filter_content(&self.filter_text);
                }
            },
        );

        if self.requested_focus {
            ui.ctx()
                .memory_mut(|mem| mem.request_focus(ID_SEARCH.into()));
            self.requested_focus = false;
        }

        if ui.ctx().input(|i| i.key_pressed(egui::Key::ArrowUp)) {
            self.selector.select_prev();
            self.requested_scroll = true;
        }
        if ui.ctx().input(|i| i.key_pressed(egui::Key::ArrowDown)) {
            self.selector.select_next();
            self.requested_scroll = true;
        }

        if ui.ctx().input(|i| i.key_pressed(egui::Key::Enter)) {
            let selected = self.selector.get_selection().cloned();
            if let Some(selected) = selected {
                self.select(&selected);
            } else {
                // Select the first one
            };
        }
    }
}

struct Selector {
    elements: Arc<Mutex<Vec<SelectorEntry>>>,
    filtered_elements: Vec<SelectorEntry>,
    selected: Option<usize>,
    tx: mpsc::Sender<Vec<SelectorEntry>>,
    rx: mpsc::Receiver<Vec<SelectorEntry>>,
}

impl Selector {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            elements: Arc::new(Mutex::new(vec![])),
            filtered_elements: vec![],
            selected: None,
            tx,
            rx,
        }
    }

    pub fn get_selection(&self) -> Option<&SelectorEntry> {
        if let Some(selected) = self.selected {
            self.filtered_elements.get(selected)
        } else {
            None
        }
    }

    pub fn get_selected(&self) -> Option<usize> {
        self.selected
    }

    pub fn set_selected(&mut self, number: Option<usize>) {
        if self.filtered_elements.is_empty() {
            self.selected = None;
        } else {
            self.selected = number.map(|n| std::cmp::min(self.filtered_elements.len() - 1, n));
        }
    }

    pub fn select_next(&mut self) {
        if self.filtered_elements.is_empty() {
            self.selected = None;
        } else {
            self.selected = Some(if let Some(mut selected) = self.selected {
                selected += 1;
                if selected > self.filtered_elements.len() - 1 {
                    selected - self.filtered_elements.len()
                } else {
                    selected
                }
            } else {
                0
            });
        }
    }

    pub fn select_prev(&mut self) {
        if self.filtered_elements.is_empty() {
            self.selected = None;
        } else {
            self.selected = Some(if let Some(mut selected) = self.selected {
                if selected == 0 {
                    selected = self.filtered_elements.len() - 1;
                } else {
                    selected -= 1;
                }
                selected
            } else {
                0
            });
        }
    }

    pub fn clear(&mut self) {
        self.elements.lock().unwrap().clear();
        self.filtered_elements.clear();
    }

    fn filter_content<S: AsRef<str>>(&mut self, filter_text: S) {
        let tx = self.tx.clone();
        let elements = Arc::clone(&self.elements);
        let filter_text = filter_text.as_ref().to_owned();
        std::thread::spawn(move || {
            let mut matcher = nucleo::Matcher::new(nucleo::Config::DEFAULT);
            let filtered = nucleo::pattern::Pattern::parse(
                &filter_text,
                nucleo::pattern::CaseMatching::Ignore,
                nucleo::pattern::Normalization::Smart,
            )
            .match_list(elements.lock().unwrap().iter(), &mut matcher)
            .iter()
            .map(|e| e.0.to_owned())
            .collect::<Vec<SelectorEntry>>();

            if let Err(e) = tx.send(filtered) {
                error!("Error sending filtered results: {}", e)
            }
        });
    }

    fn update_elements(&mut self) {
        if let Some(elements) = self.rx.try_iter().last() {
            self.filtered_elements = elements;
            self.filtered_elements
                .par_sort_by(|a, b| a.get_sort_string().cmp(&b.get_sort_string()));
        }
    }

    fn get_elements(&self) -> &Vec<SelectorEntry> {
        &self.filtered_elements
    }
}

#[derive(Clone)]
pub struct SelectorEntry {
    path: NotePath,
    path_str: String,
    entry_type: SelectorEntryType,
}

#[derive(Clone)]
enum SelectorEntryType {
    Note { title: String },
    Directory,
    Attachment,
}

impl From<SearchResult> for SelectorEntry {
    fn from(value: SearchResult) -> Self {
        match value {
            SearchResult::Note(note_details) => SelectorEntry {
                path: note_details.path.clone(),
                path_str: note_details.path.get_parent_path().1,
                entry_type: SelectorEntryType::Note {
                    title: note_details.get_title(),
                },
            },
            SearchResult::Directory(directory_details) => SelectorEntry {
                path: directory_details.path.clone(),
                path_str: directory_details.path.get_parent_path().1,
                entry_type: SelectorEntryType::Directory,
            },
            SearchResult::Attachment(path) => SelectorEntry {
                path: path.clone(),
                path_str: path.get_parent_path().1,
                entry_type: SelectorEntryType::Attachment,
            },
        }
    }
}

impl SelectorEntry {
    fn get_label(&self, ui: &mut egui::Ui) -> egui::Response {
        let icon = match &self.entry_type {
            SelectorEntryType::Note { title: _ } => icons::NOTE,
            SelectorEntryType::Directory => icons::DIRECTORY,
            SelectorEntryType::Attachment => icons::ATTACHMENT,
        };
        ui.label(format!("{}   {}", icon, self.path_str))
    }

    fn get_sort_string(&self) -> String {
        match &self.entry_type {
            SelectorEntryType::Note { title: _ } => format!("2{}", self.path),
            SelectorEntryType::Directory => format!("1{}", self.path),
            SelectorEntryType::Attachment => format!("3{}", self.path),
        }
    }
}

impl AsRef<str> for SelectorEntry {
    fn as_ref(&self) -> &str {
        &self.path_str
    }
}