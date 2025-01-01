use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{mpsc::Sender, Arc, Mutex},
};

use ignore::{ParallelVisitor, ParallelVisitorBuilder};
use log::error;

use crate::{
    nfs::{DirectoryDetails, EntryData, NoteDetails, NoteEntryData, NotePath, VaultEntry},
    NotesValidation, SearchResult,
};

struct NoteListVisitor {
    workspace_path: PathBuf,
    validation: NotesValidation,
    notes_to_delete: Arc<Mutex<HashMap<NotePath, (NoteEntryData, NoteDetails)>>>,
    notes_to_modify: Arc<Mutex<Vec<(NoteEntryData, NoteDetails)>>>,
    notes_to_add: Arc<Mutex<Vec<(NoteEntryData, NoteDetails)>>>,
    directories_found: Arc<Mutex<Vec<NotePath>>>,
    sender: Option<Sender<SearchResult>>,
}

impl NoteListVisitor {
    fn verify_cache(&self, entry: &VaultEntry) {
        let result = match &entry.data {
            EntryData::Note(note_data) => SearchResult::Note(self.verify_cached_note(note_data)),
            EntryData::Directory(directory_data) => {
                let details = DirectoryDetails {
                    path: directory_data.path.clone(),
                };
                self.directories_found
                    .lock()
                    .unwrap()
                    .push(directory_data.path.clone());
                SearchResult::Directory(details)
            }
            EntryData::Attachment => SearchResult::Attachment(entry.path.clone()),
        };
        if let Some(sender) = &self.sender {
            if let Err(e) = sender.send(result) {
                error!("{}", e)
            }
        }
    }

    fn has_changed_fast_check(&self, cached: &NoteEntryData, disk: &NoteEntryData) -> bool {
        let modified_secs = disk.modified_secs;
        let size = disk.size;
        let modified_sec_cached = cached.modified_secs;
        let size_cached = cached.size;
        size != size_cached || modified_secs != modified_sec_cached
    }

    fn has_changed_deep_check(&self, cached: &mut NoteDetails, disk: &NoteEntryData) -> bool {
        let details = disk.load_details(&self.workspace_path, &disk.path).unwrap();
        let details_hash = details.data.hash;
        let cached_hash = cached.data.hash;
        !details_hash.eq(&cached_hash)
    }

    fn verify_cached_note(&self, data: &NoteEntryData) -> NoteDetails {
        let mut ntd = self.notes_to_delete.lock().unwrap();
        let cached_option = ntd.remove(&data.path);

        let details = if let Some((cached_data, mut cached_details)) = cached_option {
            // entry exists
            let changed = match self.validation {
                NotesValidation::Full => self.has_changed_deep_check(&mut cached_details, data),
                NotesValidation::Fast => self.has_changed_fast_check(&cached_data, data),
                NotesValidation::None => false,
            };
            if changed {
                let details = data
                    .load_details(&self.workspace_path, &data.path)
                    .expect("Can't get details for note");
                self.notes_to_modify
                    .lock()
                    .unwrap()
                    .push((data.to_owned(), details.to_owned()));
                details
            } else {
                cached_details
            }
        } else {
            let details = data
                .load_details(&self.workspace_path, &data.path)
                .expect("Can't get Details for note");
            self.notes_to_add
                .lock()
                .unwrap()
                .push((data.to_owned(), details.to_owned()));
            details
        };
        details
    }
}

impl ParallelVisitor for NoteListVisitor {
    fn visit(&mut self, entry: Result<ignore::DirEntry, ignore::Error>) -> ignore::WalkState {
        match entry {
            Ok(dir) => {
                // debug!("Scanning: {}", dir.path().as_os_str().to_string_lossy());
                let npe = VaultEntry::from_path(&self.workspace_path, dir.path());
                match npe {
                    Ok(entry) => {
                        self.verify_cache(&entry);
                    }
                    Err(e) => {
                        error!("{}", e);
                    }
                }
                ignore::WalkState::Continue
            }
            Err(e) => {
                error!("{}", e);
                ignore::WalkState::Continue
            }
        }
    }
}

pub struct NoteListVisitorBuilder {
    workspace_path: PathBuf,
    validation: NotesValidation,
    notes_to_delete: Arc<Mutex<HashMap<NotePath, (NoteEntryData, NoteDetails)>>>,
    notes_to_modify: Arc<Mutex<Vec<(NoteEntryData, NoteDetails)>>>,
    notes_to_add: Arc<Mutex<Vec<(NoteEntryData, NoteDetails)>>>,
    directories_found: Arc<Mutex<Vec<NotePath>>>,
    sender: Option<Sender<SearchResult>>,
}

impl NoteListVisitorBuilder {
    pub fn new<P: AsRef<Path>>(
        workspace_path: P,
        validation: NotesValidation,
        cached_notes: Vec<(NoteEntryData, NoteDetails)>,
        sender: Option<Sender<SearchResult>>,
    ) -> Self {
        let mut notes_to_delete = HashMap::new();
        for cached in cached_notes {
            let path = cached.1.path.clone();
            notes_to_delete.insert(path, cached);
        }
        Self {
            workspace_path: workspace_path.as_ref().to_path_buf(),
            validation,
            notes_to_delete: Arc::new(Mutex::new(notes_to_delete)),
            notes_to_modify: Arc::new(Mutex::new(Vec::new())),
            notes_to_add: Arc::new(Mutex::new(Vec::new())),
            directories_found: Arc::new(Mutex::new(Vec::new())),
            sender,
        }
    }

    pub fn get_notes_to_delete(&self) -> Vec<NotePath> {
        self.notes_to_delete
            .lock()
            .unwrap()
            .iter()
            .map(|n| n.0.to_owned())
            .collect()
    }

    pub fn get_notes_to_add(&self) -> Vec<(NoteEntryData, NoteDetails)> {
        self.notes_to_add
            .lock()
            .unwrap()
            .iter()
            .map(|n| n.to_owned())
            .collect()
    }

    pub fn get_notes_to_modify(&self) -> Vec<(NoteEntryData, NoteDetails)> {
        self.notes_to_modify
            .lock()
            .unwrap()
            .iter()
            .map(|n| n.to_owned())
            .collect()
    }

    pub fn get_directories_found(&self) -> Vec<NotePath> {
        self.directories_found
            .lock()
            .unwrap()
            .iter()
            .map(|n| n.to_owned())
            .collect()
    }
}

impl<'s> ParallelVisitorBuilder<'s> for NoteListVisitorBuilder {
    fn build(&mut self) -> Box<dyn ParallelVisitor + 's> {
        let dbv = NoteListVisitor {
            workspace_path: self.workspace_path.clone(),
            validation: self.validation.clone(),
            notes_to_delete: self.notes_to_delete.clone(),
            notes_to_modify: self.notes_to_modify.clone(),
            notes_to_add: self.notes_to_add.clone(),
            directories_found: self.directories_found.clone(),
            sender: self.sender.clone(),
        };
        Box::new(dbv)
    }
}