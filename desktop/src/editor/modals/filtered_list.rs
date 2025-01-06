use std::{
    collections::VecDeque,
    sync::{mpsc, Arc},
};

use eframe::egui;
use log::{debug, error, info};
use notes_core::{nfs::NotePath, SearchResult};

use crate::icons;

use super::{EditorMessage, EditorModal};

pub const ID_SEARCH: &str = "Search Popup";

#[derive(Debug)]
enum SelectorState<P, D>
where
    D: Send + Clone + 'static,
    P: Send + Sync + Clone + 'static,
{
    Initializing,
    Initialized { provider: P },
    Filtering,
    Filtered { filter: String, data: D },
    Ready { filter: String },
}

impl<P, D> std::fmt::Display for SelectorState<P, D>
where
    D: Send + Clone + 'static,
    P: Send + Sync + Clone + 'static,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SelectorState::Initializing => write!(f, "Initializing"),
            SelectorState::Initialized { provider: _ } => write!(f, "Initialized"),
            SelectorState::Filtering => write!(f, "Filtering"),
            SelectorState::Filtered { filter, data: _ } => {
                write!(f, "Filtered with filter `{}`", filter)
            }
            SelectorState::Ready { filter } => {
                write!(f, "Ready with filter `{}`", filter)
            }
        }
    }
}

pub trait FilteredListFunctions<P, D>: Clone + Send {
    fn init(&self) -> P;
    fn filter<S: AsRef<str>>(&self, filter_text: S, provider: &P) -> D;
    fn get_elements(&self, data: &D) -> Vec<SelectorEntry>;
    fn on_entry(&mut self, element: &SelectorEntry) -> Option<FilteredListFunctionMessage>;
}

pub enum FilteredListFunctionMessage {
    ToEditor(EditorMessage),
    ResetState,
}

pub(super) struct FilteredList<F, P, D>
where
    F: FilteredListFunctions<P, D> + 'static,
    D: Send + Clone + 'static,
    P: Send + Sync + Clone + 'static,
{
    state_manager: SelectorStateManager<F, P, D>,
    message_sender: mpsc::Sender<EditorMessage>,
    requested_clear: bool,
    requested_focus: bool,
    requested_scroll: bool,
}

impl<F, P, D> FilteredList<F, P, D>
where
    F: FilteredListFunctions<P, D> + 'static,
    D: Send + Clone + 'static,
    P: Send + Sync + Clone + 'static,
{
    pub fn new(functions: F, message_sender: mpsc::Sender<EditorMessage>) -> Self {
        let mut state_manager = SelectorStateManager::new(functions);
        state_manager.initialize();
        Self {
            state_manager,
            message_sender,
            requested_clear: false,
            requested_focus: true,
            requested_scroll: false,
        }
    }

    pub fn request_focus(&mut self) {
        self.requested_focus = true;
    }

    fn select(&mut self, selected: &SelectorEntry) {
        if let Some(message) = self.state_manager.functions.on_entry(selected) {
            match message {
                FilteredListFunctionMessage::ToEditor(editor_message) => {
                    if let Err(e) = self.message_sender.send(editor_message) {
                        error!("Can't send the message to editor, Err: {}", e)
                    }
                }
                FilteredListFunctionMessage::ResetState => {
                    self.request_focus();
                    if let Err(e) = self.state_manager.tx.send(SelectorState::Initializing) {
                        error!("Can't reset the state, Err: {}", e)
                    }
                }
            }
        }
    }
}

impl<F, P, D> EditorModal for FilteredList<F, P, D>
where
    F: FilteredListFunctions<P, D>,
    D: Send + Clone + 'static,
    P: Send + Sync + Clone + 'static,
{
    fn update(&mut self, ui: &mut egui::Ui) {
        if self.requested_clear {
            self.state_manager.clear();
            self.requested_clear = false;
        }

        self.state_manager.update();

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
                let _filter_response = ui.add(
                    egui::TextEdit::singleline(&mut self.state_manager.filter_text)
                        .desired_width(f32::INFINITY)
                        .id(ID_SEARCH.into()),
                );

                let mut selected = self.state_manager.get_selected();
                let scroll_area = egui::scroll_area::ScrollArea::vertical()
                    .max_height(400.0)
                    .auto_shrink(false);
                scroll_area.show(ui, |ui| {
                    ui.vertical(|ui| {
                        // TODO: sadly we need to clone here, so we may have some innefficiencies
                        let elements = self.state_manager.get_elements().to_owned();
                        for (pos, element) in elements.iter().enumerate() {
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
                self.state_manager.set_selected(selected);
            },
        );

        if self.requested_focus {
            ui.ctx()
                .memory_mut(|mem| mem.request_focus(ID_SEARCH.into()));
            self.requested_focus = false;
        }

        if ui.ctx().input(|i| i.key_pressed(egui::Key::ArrowUp)) {
            self.state_manager.select_prev();
            self.requested_scroll = true;
        }
        if ui.ctx().input(|i| i.key_pressed(egui::Key::ArrowDown)) {
            self.state_manager.select_next();
            self.requested_scroll = true;
        }

        if ui.ctx().input(|i| i.key_pressed(egui::Key::Enter)) {
            let selected = self.state_manager.get_selection();
            if let Some(selected) = selected {
                self.select(&selected);
            } else {
                // Select the first one
            };
        }
    }
}

struct SelectorStateManager<F, P, D>
where
    F: FilteredListFunctions<P, D> + 'static,
    D: Send + Clone + 'static,
    P: Send + Sync + Clone + 'static,
{
    state: SelectorState<P, D>,
    filter_text: String,
    provider: Option<Arc<P>>,
    state_data: Vec<SelectorEntry>,
    functions: F,
    selected: Option<usize>,
    tx: mpsc::Sender<SelectorState<P, D>>,
    rx: mpsc::Receiver<SelectorState<P, D>>,
    deduped_message_bus: VecDeque<SelectorState<P, D>>,
}

impl<F, P, D> SelectorStateManager<F, P, D>
where
    F: FilteredListFunctions<P, D> + 'static,
    D: Send + Clone + 'static,
    P: Send + Sync + Clone + 'static,
{
    pub fn new(functions: F) -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            state: SelectorState::Initializing,
            filter_text: String::new(),
            provider: None,
            state_data: vec![],
            functions,
            selected: None,
            tx,
            rx,
            deduped_message_bus: VecDeque::new(),
        }
    }

    pub fn initialize(&mut self) {
        debug!("Initializing");
        self.state_data.clear();
        let tx = self.tx.clone();
        let functions = self.functions.clone();
        std::thread::spawn(move || {
            let provider = functions.init();
            if let Err(e) = tx.send(SelectorState::Initialized { provider }) {
                error!("Error sending initialized status: {}", e);
            }
        });
    }

    fn trigger_filter(&mut self) {
        if let Some(provider_arc) = &self.provider {
            self.state = SelectorState::Filtering;
            let tx = self.tx.clone();
            let functions = self.functions.clone();
            let filter_text = self.filter_text.clone();
            let provider = Arc::clone(provider_arc);
            std::thread::spawn(move || {
                info!("Applying filter");
                let data = functions.filter(filter_text.clone(), &provider);
                if let Err(e) = tx.send(SelectorState::Filtered {
                    filter: filter_text,
                    data,
                }) {
                    error!("Error sending ready status: {}", e);
                }
            });
        } else {
            panic!(
                "Wrong state, no provider present, current state is: {}",
                self.state
            );
        }
    }

    pub fn clear(&self) {
        if let Err(e) = self.tx.send(SelectorState::Initializing) {
            error!("Error sending a clear message {}", e);
        }
    }

    pub fn get_elements(&self) -> &Vec<SelectorEntry> {
        &self.state_data
    }

    pub fn get_selection(&self) -> Option<SelectorEntry> {
        if let Some(selected) = self.selected {
            let elements = self.get_elements();
            let sel = elements.get(selected);
            sel.cloned()
        } else {
            None
        }
    }

    pub fn get_selected(&self) -> Option<usize> {
        self.selected
    }

    pub fn set_selected(&mut self, number: Option<usize>) {
        let elements = self.get_elements();
        if !elements.is_empty() {
            self.selected = number.map(|n| std::cmp::min(elements.len() - 1, n));
        } else {
            self.selected = None;
        }
    }

    pub fn select_next(&mut self) {
        let elements = self.get_elements();
        if !elements.is_empty() {
            self.selected = Some(if let Some(mut selected) = self.selected {
                selected += 1;
                if selected > elements.len() - 1 {
                    selected - elements.len()
                } else {
                    selected
                }
            } else {
                0
            });
        } else {
            self.selected = None;
        }
    }

    pub fn select_prev(&mut self) {
        let elements = self.get_elements();
        if !elements.is_empty() {
            self.selected = Some(if let Some(mut selected) = self.selected {
                if selected == 0 {
                    selected = elements.len() - 1;
                } else {
                    selected -= 1;
                }
                selected
            } else {
                0
            });
        } else {
            self.selected = None;
        }
    }

    fn update(&mut self) {
        // We make sure we don't trigger two equal state changes consecutively
        // this is especially relevant for the filters, so if a filter function
        // takes a little, we don't want to stack filter changes if the text
        // of the filter changes faster than the actual results
        while let Ok(state) = self.rx.try_recv() {
            if let Some(queued_state) = self.deduped_message_bus.back() {
                if core::mem::discriminant(queued_state) != core::mem::discriminant(&state) {
                    self.deduped_message_bus.push_back(state);
                } else {
                    debug!(
                        "Duplicated state events so we are replacing the last one in the queue: {}",
                        state
                    );
                    self.deduped_message_bus.pop_back();
                    self.deduped_message_bus.push_back(state);
                }
            } else {
                self.deduped_message_bus.push_back(state);
            }
        }
        if let Some(state) = self.deduped_message_bus.pop_front() {
            info!("New Status received: {}", state);
            self.state = state;
            match &self.state {
                SelectorState::Initializing => {
                    info!("Status is clear, we initialize");
                    self.initialize()
                }
                SelectorState::Initialized { provider } => {
                    info!("Status initialized, we proceed to apply filter");
                    // Only place we need to clone the provider
                    self.provider = Some(Arc::new(provider.to_owned()));
                    self.trigger_filter();
                }
                SelectorState::Filtering => {}
                SelectorState::Filtered { filter, data } => {
                    self.state_data = self.functions.get_elements(data);
                    self.state = SelectorState::Ready {
                        filter: filter.to_owned(),
                    };
                }
                SelectorState::Ready { filter: _ } => {}
            }
        }
        if let SelectorState::Ready { filter } = &self.state {
            // We are ready to show elements
            if filter != &self.filter_text {
                info!("Filter changed, we reapply the filter");
                self.trigger_filter();
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct SelectorEntry {
    pub path: NotePath,
    pub path_str: String,
    pub entry_type: SelectorEntryType,
}

#[derive(Clone, Debug)]
pub enum SelectorEntryType {
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
        match &self.entry_type {
            SelectorEntryType::Note { title } => {
                let icon = icons::NOTE;
                let path = self.path_str.to_owned();
                ui.label(format!("{}  {}\n{}", icon, title, path))
                // let mut job = egui::text::LayoutJob::default();
                // job.append(
                //     format!("{}   {}\n", icon, title).as_str(),
                //     0.0,
                //     egui::TextFormat::default(),
                // );
                // job.append(
                //     path.as_str(),
                //     0.0,
                //     egui::TextFormat {
                //         italics: true,
                //         ..Default::default()
                //     },
                // );
                // ui.label(job)
            }
            SelectorEntryType::Directory => {
                let icon = icons::DIRECTORY;
                let path = self.path_str.to_owned();
                ui.label(format!("{}  {}", icon, path))
                // let mut job = egui::text::LayoutJob::default();
                // job.append(
                //     format!("{}   {}", icon, self.path_str).as_str(),
                //     0.0,
                //     egui::TextFormat::default(),
                // );
                // ui.label(job)
            }
            SelectorEntryType::Attachment => {
                let icon = icons::ATTACHMENT;
                let path = self.path_str.to_owned();
                ui.label(format!("{}  {}", icon, path))
                // let mut job = egui::text::LayoutJob::default();
                // job.append(
                //     format!("{}   {}", icon, self.path_str).as_str(),
                //     0.0,
                //     egui::TextFormat::default(),
                // );
                // ui.label(job)
            }
        }
    }

    pub fn get_sort_string(&self) -> String {
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
