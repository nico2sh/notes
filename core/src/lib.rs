mod content_data;
mod db;
pub mod error;
pub mod nfs;
pub mod utilities;

use std::{
    collections::HashSet,
    fmt::Display,
    path::{Path, PathBuf},
    sync::mpsc::{Receiver, Sender},
};

use chrono::Utc;
use content_data::NoteContentData;
use db::VaultDB;
// use db::async_sqlite::AsyncConnection;
// use db::async_db::AsyncConnection;
use error::{DBError, FSError, VaultError};
use log::{debug, info};
use nfs::{load_note, save_note, visitor::NoteListVisitorBuilder, VaultEntry, VaultPath};
use utilities::path_to_string;

const JOURNAL_PATH: &str = "journal";

#[derive(Debug, Clone, PartialEq)]
pub struct NoteVault {
    pub workspace_path: PathBuf,
    vault_db: VaultDB,
}

impl NoteVault {
    /// Creates a new instance of the Note Vault.
    /// Make sure you call `NoteVault::init_and_validate(&self)` to initialize the DB index if
    /// needed
    pub fn new<P: AsRef<Path>>(workspace_path: P) -> Result<Self, VaultError> {
        debug!("Creating new vault Instance");
        let workspace_path = workspace_path.as_ref().to_path_buf();
        if !workspace_path.exists() {
            return Err(VaultError::VaultPathNotFound {
                path: path_to_string(workspace_path),
            })?;
        }
        if !workspace_path.is_dir() {
            return Err(VaultError::FSError(FSError::InvalidPath {
                path: path_to_string(workspace_path),
            }))?;
        };

        let vault_db = VaultDB::new(&workspace_path);
        let note_vault = Self {
            workspace_path,
            vault_db,
        };
        Ok(note_vault)
    }

    /// On init and validate it verifies the DB index to make sure:
    ///
    /// 1. It exists
    /// 2. It is valid.
    /// 3. Its schema is updated
    ///
    /// Then does a quick scan of the workspace directory to update the index if there are new or
    /// missing notes.
    /// This can be slow on large vaults.
    pub fn init_and_validate(&self) -> Result<(), VaultError> {
        debug!("Initializing DB and validating it");
        let db_path = self.vault_db.get_db_path();
        let db_result = self.vault_db.check_db()?;
        match db_result {
            db::DBStatus::Ready => {
                // We only check if there are new notes
                self.index_notes(NotesValidation::None)?;
            }
            db::DBStatus::Outdated => {
                self.recreate_index()?;
            }
            db::DBStatus::NotValid => {
                let md = std::fs::metadata(&db_path).map_err(FSError::ReadFileError)?;
                if md.is_dir() {
                    std::fs::remove_dir_all(db_path).map_err(FSError::ReadFileError)?;
                } else {
                    std::fs::remove_file(db_path).map_err(FSError::ReadFileError)?;
                }
                self.recreate_index()?;
            }
            db::DBStatus::FileNotFound => {
                // No need to validate, no data there
                self.create_tables()?;
                self.index_notes(NotesValidation::None)?;
            }
        }
        Ok(())
    }

    /// Deletes all the cached data from the DB
    /// and recreates the index
    pub fn recreate_index(&self) -> Result<(), VaultError> {
        debug!("Initializing DB from Vault request");
        self.create_tables()?;
        debug!("Tables created, creating index");
        self.index_notes(NotesValidation::Full)?;
        Ok(())
    }

    fn create_tables(&self) -> Result<(), VaultError> {
        self.vault_db.call(db::init_db)?;
        Ok(())
    }

    /// Traverses the whole vault directory and verifies the notes to
    /// update the cached data in the DB. The validation is defined by
    /// the validation mode:
    ///
    /// NotesValidation::Full Checks the content of the note by comparing a hash based on the text
    /// conatined in the file.
    /// NotesValidation::Fast Checks the size of the file to identify if the note has changed and
    /// then update the DB entry.
    /// NotesValidation::None Checks if the note exists or not.
    pub fn index_notes(&self, validation_mode: NotesValidation) -> Result<(), VaultError> {
        info!("Start indexing files");
        let start = std::time::SystemTime::now();
        let workspace_path = self.workspace_path.clone();
        self.vault_db.call(move |conn| {
            create_index_for(&workspace_path, conn, &VaultPath::root(), validation_mode)
        })?;

        let time = std::time::SystemTime::now()
            .duration_since(start)
            .expect("Something's wrong with the time");
        info!(
            "Files indexed in the DB in {} milliseconds",
            time.as_millis()
        );
        Ok(())
    }

    pub fn exists(&self, path: &VaultPath) -> Option<VaultEntry> {
        match VaultEntry::new(&self.workspace_path, path.to_owned()) {
            Ok(entry) => Some(entry),
            Err(_e) => None,
        }
    }

    pub fn journal_entry(&self) -> Result<(NoteDetails, String), VaultError> {
        let (title, note_path) = self.get_todays_journal();
        let content = self.load_or_create_note(&note_path, Some(format!("# {}\n\n", title)))?;
        let details = NoteDetails::from_content(&content, &note_path);
        Ok((details, content))
    }

    fn get_todays_journal(&self) -> (String, VaultPath) {
        let today = Utc::now();
        let today_string = today.format("%Y-%m-%d").to_string();

        (
            today_string.clone(),
            VaultPath::from(JOURNAL_PATH).append(&VaultPath::file_from(&today_string)),
        )
    }

    // Loads a note in the specified path, if the path doesn't exist
    // create a new one, a text can be specified as the initial text for the
    // note when created
    pub fn load_or_create_note(
        &self,
        path: &VaultPath,
        default_text: Option<String>,
    ) -> Result<String, VaultError> {
        match load_note(&self.workspace_path, path) {
            Ok(text) => Ok(text),
            Err(e) => {
                if let FSError::VaultPathNotFound { path: _ } = e {
                    let text = default_text.unwrap_or_default();
                    self.create_note(path, &text)?;
                    Ok(text)
                } else {
                    Err(e)?
                }
            }
        }
    }

    // Loads the note's content, returns the text
    // If the file doesn't exist you will get a VaultError::FSError with a
    // FSError::NotePathNotFound as the source, you can use that to
    // lazy create a note, or use the load_or_create_note function instead
    pub fn load_note(&self, path: &VaultPath) -> Result<String, VaultError> {
        let text = load_note(&self.workspace_path, path)?;
        Ok(text)
    }

    // Search notes using terms
    pub fn search_notes<S: AsRef<str>>(
        &self,
        terms: S,
        wildcard: bool,
    ) -> Result<Vec<NoteDetails>, VaultError> {
        // let mut connection = ConnectionBuilder::new(&self.workspace_path)
        //     .build()
        //     .unwrap();
        let terms = terms.as_ref().to_owned();

        let a = self.vault_db.call(move |conn| {
            db::search_terms(conn, terms, wildcard).map(|vec| {
                vec.into_iter()
                    .map(|(_data, details)| details)
                    .collect::<Vec<NoteDetails>>()
            })
        })?;

        Ok(a)
    }

    pub fn browse_vault(&self, options: VaultBrowseOptions) -> Result<(), VaultError> {
        let start = std::time::SystemTime::now();
        debug!("> Start fetching files with Options:\n{}", options);

        // TODO: See if we can put everything inside the closure
        let query_path = options.path.clone();
        let cached_notes = self.vault_db.call(move |conn| {
            let notes = db::get_notes(conn, &query_path, options.recursive)?;
            Ok(notes)
        })?;

        let mut builder = NoteListVisitorBuilder::new(
            &self.workspace_path,
            options.validation,
            cached_notes,
            Some(options.sender.clone()),
        );
        // We traverse the directory
        let walker = nfs::get_file_walker(
            self.workspace_path.clone(),
            &options.path,
            options.recursive,
        );
        walker.visit(&mut builder);

        let notes_to_add = builder.get_notes_to_add();
        let notes_to_delete = builder.get_notes_to_delete();
        let notes_to_modify = builder.get_notes_to_modify();

        let workspace_path = self.workspace_path.clone();
        self.vault_db.call(move |conn| {
            let tx = conn.transaction()?;
            db::insert_notes(&tx, &workspace_path, &notes_to_add)?;
            db::delete_notes(&tx, &notes_to_delete)?;
            db::update_notes(&tx, &workspace_path, &notes_to_modify)?;
            tx.commit()?;
            Ok(())
        })?;

        let time = std::time::SystemTime::now()
            .duration_since(start)
            .expect("Something's wrong with the time");
        debug!("> Files fetched in {} milliseconds", time.as_millis());

        Ok(())
    }

    pub fn get_notes(
        &self,
        path: &VaultPath,
        recursive: bool,
    ) -> Result<Vec<NoteDetails>, VaultError> {
        let start = std::time::SystemTime::now();
        debug!("> Start fetching files from cache");
        let note_path = path.into();

        let cached_notes = self.vault_db.call(move |conn| {
            let notes = db::get_notes(conn, &note_path, recursive)?;
            Ok(notes)
        })?;

        let result = cached_notes
            .iter()
            .map(|(_data, details)| details.to_owned())
            .collect::<Vec<NoteDetails>>();
        let time = std::time::SystemTime::now()
            .duration_since(start)
            .expect("Something's wrong with the time");
        debug!("> Files fetched in {} milliseconds", time.as_millis());
        Ok(result)
    }

    pub fn create_note<S: AsRef<str>>(&self, path: &VaultPath, text: S) -> Result<(), VaultError> {
        if self.exists(path).is_none() {
            self.save_note(path, text)
        } else {
            Err(VaultError::NoteExists { path: path.clone() })
        }
    }

    pub fn save_note<S: AsRef<str>>(&self, path: &VaultPath, text: S) -> Result<(), VaultError> {
        // Save to disk
        let entry_data = save_note(&self.workspace_path, path, &text)?;

        let details = entry_data.load_details(&self.workspace_path, path)?;

        // Save to DB
        let text = text.as_ref().to_owned();
        self.vault_db
            .call(move |conn| db::save_note(conn, text, &entry_data, &details))?;

        Ok(())
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct NoteDetails {
    pub path: VaultPath,
    pub data: NoteContentData,
    // Content may be lazy fetched
    // if the details are taken from the DB, the content is
    // likely not going to be there, so the `get_content` function
    // will take it from disk, and store in the cache
    cached_text: Option<String>,
}

impl Display for NoteDetails {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Path: {}, Data: {}, Has Text Cached: {}",
            self.path,
            self.data,
            self.cached_text.is_some()
        )
    }
}

impl NoteDetails {
    pub fn new(note_path: VaultPath, hash: u64, title: String, text: Option<String>) -> Self {
        let data = NoteContentData {
            hash,
            title: Some(title),
            content_chunks: vec![],
        };
        Self {
            path: note_path,
            data,
            cached_text: text,
        }
    }

    fn from_content<S: AsRef<str>>(text: S, note_path: &VaultPath) -> Self {
        let data = content_data::extract_data(&text);
        Self {
            path: note_path.to_owned(),
            data,
            cached_text: Some(text.as_ref().to_owned()),
        }
    }

    pub fn get_text<P: AsRef<Path>>(&mut self, base_path: P) -> Result<String, VaultError> {
        let content = self.cached_text.clone();
        // Content may be lazy loaded from disk since it's
        // the only data that is not stored in the DB
        if let Some(content) = content {
            Ok(content)
        } else {
            let content = load_note(base_path, &self.path)?;
            self.cached_text = Some(content.clone());
            Ok(content)
        }
    }

    pub fn get_title(&self) -> String {
        self.data
            .title
            .clone()
            .unwrap_or_else(|| self.path.get_parent_path().1)
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct DirectoryDetails {
    pub path: VaultPath,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SearchResult {
    Note(NoteDetails),
    Directory(DirectoryDetails),
    Attachment(VaultPath),
}

fn collect_from_cache(
    cached_notes: &[(nfs::NoteEntryData, NoteDetails)],
) -> Result<Vec<SearchResult>, VaultError> {
    let mut directories = HashSet::new();
    let mut notes = vec![];

    for (_note_data, note_details) in cached_notes {
        directories.insert(note_details.path.get_parent_path().0);
        notes.push(SearchResult::Note(note_details.clone()));
    }

    let result = directories
        .iter()
        .map(|directory_path| {
            SearchResult::Directory(DirectoryDetails {
                path: directory_path.clone(),
            })
        })
        .chain(notes);
    Ok(result.collect())
}

pub struct VaultBrowseOptionsBuilder {
    path: VaultPath,
    validation: NotesValidation,
    recursive: bool,
}

impl VaultBrowseOptionsBuilder {
    pub fn new(path: &VaultPath) -> Self {
        Self::default().path(path.clone())
    }

    pub fn build(self) -> (VaultBrowseOptions, Receiver<SearchResult>) {
        let (sender, receiver) = std::sync::mpsc::channel();
        (
            VaultBrowseOptions {
                path: self.path,
                validation: self.validation,
                recursive: self.recursive,
                sender,
            },
            receiver,
        )
    }

    pub fn path(mut self, path: VaultPath) -> Self {
        self.path = path;
        self
    }

    pub fn recursive(mut self) -> Self {
        self.recursive = true;
        self
    }

    pub fn non_recursive(mut self) -> Self {
        self.recursive = false;
        self
    }

    pub fn full_validation(mut self) -> Self {
        self.validation = NotesValidation::Full;
        self
    }

    pub fn fast_validation(mut self) -> Self {
        self.validation = NotesValidation::Fast;
        self
    }

    pub fn no_validation(mut self) -> Self {
        self.validation = NotesValidation::None;
        self
    }
}

impl Default for VaultBrowseOptionsBuilder {
    fn default() -> Self {
        Self {
            path: VaultPath::root(),
            validation: NotesValidation::None,
            recursive: false,
        }
    }
}

#[derive(Debug, Clone)]
/// Options to traverse the Notes
/// You need a sync::mpsc::Sender to use a channel to receive the entries
pub struct VaultBrowseOptions {
    path: VaultPath,
    validation: NotesValidation,
    recursive: bool,
    sender: Sender<SearchResult>,
}

impl Display for VaultBrowseOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Vault Browse Options - [Path: `{}`|Validation Type: `{}`|Recursive: `{}`]",
            self.path, self.validation, self.recursive
        )
    }
}

#[derive(Debug, Clone, Copy)]
pub enum NotesValidation {
    Full,
    Fast,
    None,
}

impl Display for NotesValidation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                NotesValidation::Full => "Full",
                NotesValidation::Fast => "Fast",
                NotesValidation::None => "None",
            }
        )
    }
}

fn create_index_for<P: AsRef<Path>>(
    workspace_path: P,
    connection: &mut rusqlite::Connection,
    path: &VaultPath,
    validation_mode: NotesValidation,
) -> Result<(), DBError> {
    debug!("Start fetching files at {}", path);
    let workspace_path = workspace_path.as_ref();
    let walker = nfs::get_file_walker(workspace_path, path, false);

    let cached_notes = db::get_notes(connection, path, false)?;
    let mut builder =
        NoteListVisitorBuilder::new(workspace_path, validation_mode, cached_notes, None);
    walker.visit(&mut builder);
    let notes_to_add = builder.get_notes_to_add();
    let notes_to_delete = builder.get_notes_to_delete();
    let notes_to_modify = builder.get_notes_to_modify();

    let tx = connection.transaction()?;
    db::delete_notes(&tx, &notes_to_delete)?;
    db::insert_notes(&tx, workspace_path, &notes_to_add)?;
    db::update_notes(&tx, workspace_path, &notes_to_modify)?;
    tx.commit()?;

    let directories_to_insert = builder.get_directories_found();
    for directory in directories_to_insert.iter().filter(|p| !p.eq(&path)) {
        create_index_for(workspace_path, connection, directory, validation_mode)?;
    }

    Ok(())
}
