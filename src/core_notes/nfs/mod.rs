pub mod visitor;
// Contains the structs to support the data types
use std::{
    ffi::OsStr,
    fmt::Display,
    io::Write,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use ignore::{WalkBuilder, WalkParallel};
use serde::{de::Visitor, Deserialize, Serialize};

use super::{error::FSError, DirectoryDetails, NoteDetails};

use super::utilities::path_to_string;

const PATH_SEPARATOR: char = '/';
const NOTE_EXTENSION: &str = ".md";
// non valid chars
const NON_VALID_PATH_CHARS_REGEX: &str = r#"[\\/:*?"<>|]"#;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VaultEntry {
    pub path: NotePath,
    pub path_string: String,
    pub data: EntryData,
}

impl AsRef<str> for VaultEntry {
    fn as_ref(&self) -> &str {
        self.path_string.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum EntryData {
    Note(NoteEntryData),
    Directory(DirectoryEntryData),
    Attachment,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NoteEntryData {
    pub path: NotePath,
    // File size, for fast check
    pub size: u64,
    pub modified_secs: u64,
}

impl NoteEntryData {
    pub fn load_details<P: AsRef<Path>>(
        &self,
        workspace_path: P,
        path: &NotePath,
    ) -> Result<NoteDetails, FSError> {
        let content = load_note(workspace_path, path)?;
        Ok(NoteDetails::from_content(content, path))
    }

    fn from_path<P: AsRef<Path>>(
        workspace_path: P,
        path: &NotePath,
    ) -> Result<NoteEntryData, FSError> {
        let file_path = path.to_pathbuf(&workspace_path);

        let metadata = file_path.metadata()?;
        let size = metadata.len();
        let modified_secs = metadata
            .modified()
            .map(|t| t.duration_since(UNIX_EPOCH).unwrap().as_secs())
            .unwrap_or_else(|_e| 0);
        Ok(NoteEntryData {
            path: path.clone(),
            size,
            modified_secs,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DirectoryEntryData {
    pub path: NotePath,
}
impl DirectoryEntryData {
    pub fn get_details<P: AsRef<Path>>(&self) -> DirectoryDetails {
        DirectoryDetails {
            path: self.path.clone(),
        }
    }
}

fn _get_dir_content_size<P: AsRef<Path>>(
    workspace_path: P,
    path: &NotePath,
) -> Result<u64, FSError> {
    let os_path = path.to_pathbuf(&workspace_path);
    let walker = ignore::WalkBuilder::new(&os_path)
        .max_depth(Some(1))
        .filter_entry(filter_files)
        .build();
    let mut content_size = 0;
    for entry in walker.flatten() {
        let entry_path = entry.path();
        if entry_path.is_file() && entry_path.extension().is_some_and(|ext| ext == "md") {
            let metadata = std::fs::metadata(&os_path)?;
            let file_size = metadata.len();
            content_size += file_size;
        }
    }
    Ok(content_size)
}

impl VaultEntry {
    pub fn new<P: AsRef<Path>>(workspace_path: P, path: NotePath) -> Result<Self, FSError> {
        let os_path = path.to_pathbuf(&workspace_path);
        if !os_path.exists() {
            return Err(FSError::NoFileOrDirectoryFound {
                path: path_to_string(os_path),
            });
        }

        let kind = if os_path.is_dir() {
            EntryData::Directory(DirectoryEntryData { path: path.clone() })
        } else if path.is_note() {
            let note_entry_data = NoteEntryData::from_path(workspace_path, &path)?;
            EntryData::Note(note_entry_data)
        } else {
            EntryData::Attachment
        };
        let path_string = path.to_string();

        Ok(VaultEntry {
            path,
            path_string,
            data: kind,
        })
    }

    pub fn from_path<P: AsRef<Path>, F: AsRef<Path>>(
        workspace_path: P,
        full_path: F,
    ) -> Result<Self, FSError> {
        let note_path = NotePath::from_path(&workspace_path, &full_path)?;
        Self::new(&workspace_path, note_path)
    }
}

impl Display for VaultEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.data {
            EntryData::Note(_details) => write!(f, "[NOT] {}", self.path),
            EntryData::Directory(_details) => write!(f, "[DIR] {}", self.path),
            EntryData::Attachment => write!(f, "[ATT]"),
        }
    }
}

#[derive(Debug, Clone)]
pub enum VaultEntryDetails {
    // Hash
    Note(NoteDetails),
    Directory(DirectoryDetails),
    None,
}

impl VaultEntryDetails {
    pub fn get_title(&mut self) -> String {
        match self {
            VaultEntryDetails::Note(note_details) => note_details.get_title(),
            VaultEntryDetails::Directory(_directory_details) => String::new(),
            VaultEntryDetails::None => String::new(),
        }
    }
}

pub fn load_note<P: AsRef<Path>>(workspace_path: P, path: &NotePath) -> Result<String, FSError> {
    let os_path = path.to_pathbuf(&workspace_path);
    let file = std::fs::read(&os_path)?;
    let text = String::from_utf8(file)?;
    Ok(text)
}

pub fn save_note<P: AsRef<Path>, S: AsRef<str>>(
    workspace_path: P,
    path: &NotePath,
    text: S,
) -> Result<NoteEntryData, FSError> {
    if !path.is_note() {
        return Err(FSError::InvalidPath {
            path: path.to_string(),
        });
    }
    let (parent, note) = path.get_parent_path();
    let base_path = parent.to_pathbuf(&workspace_path);
    let full_path = base_path.join(note);
    std::fs::create_dir_all(base_path)?;

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .create(true)
        .open(full_path)?;
    file.write_all(text.as_ref().as_bytes())?;

    let entry = NoteEntryData::from_path(workspace_path, path)?;
    Ok(entry)
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Hash)]
pub struct NotePath {
    slices: Vec<NotePathSlice>,
}

impl From<&NotePath> for NotePath {
    fn from(value: &NotePath) -> Self {
        value.to_owned()
    }
}
// impl PartialOrd for NoteEntry {
//     fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
//         match self.path_string.partial_cmp(&other.path_string) {
//             Some(core::cmp::Ordering::Equal) => None,
//             ord => return ord,
//         }
//     }
// }
//
impl Serialize for NotePath {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let string = self.to_string();
        serializer.serialize_str(string.as_ref())
    }
}

struct DeserializeNotePathVisitor;
impl Visitor<'_> for DeserializeNotePathVisitor {
    type Value = NotePath;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("A valid path with `/` separators, no need of starting `/`")
    }
    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        let path = NotePath::new(value);
        Ok(path)
    }
}

impl<'de> Deserialize<'de> for NotePath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_str(DeserializeNotePathVisitor)
    }
}

impl From<&str> for NotePath {
    fn from(value: &str) -> Self {
        NotePath::new(value)
    }
}

impl From<String> for NotePath {
    fn from(value: String) -> Self {
        NotePath::new(value)
    }
}

impl From<&String> for NotePath {
    fn from(value: &String) -> Self {
        NotePath::new(value)
    }
}

impl NotePath {
    fn new<S: AsRef<str>>(path: S) -> Self {
        let path_list = path
            .as_ref()
            .split(PATH_SEPARATOR)
            .filter(|p| !p.is_empty()) // We remove the empty ones,
            // so `//` are treated as `/`
            .map(NotePathSlice::new)
            .collect();
        Self { slices: path_list }
    }

    pub fn file_from<S: AsRef<str>>(path: S) -> Result<Self, FSError> {
        let path = path.as_ref();
        if !path.ends_with(PATH_SEPARATOR) {
            let p = if !path.ends_with(NOTE_EXTENSION) {
                [path, NOTE_EXTENSION].concat()
            } else {
                path.to_owned()
            };
            Ok(NotePath::new(p))
        } else {
            Err(FSError::InvalidPath {
                path: path.to_string(),
            })
        }
    }

    pub fn root() -> Self {
        Self { slices: Vec::new() }
    }

    pub fn get_slices(&self) -> Vec<String> {
        self.slices
            .iter()
            .map(|slice| slice.slice.to_owned())
            .collect()
    }

    fn to_pathbuf<P: AsRef<Path>>(&self, workspace_path: P) -> PathBuf {
        let mut path = workspace_path.as_ref().to_path_buf();
        for p in &self.slices {
            let slice = p.slice.clone();
            path = path.join(&slice);
        }
        path
    }

    pub fn get_name(&self) -> String {
        self.slices
            .last()
            .map_or_else(String::new, |s| s.slice.clone())
    }

    pub fn from_path<P: AsRef<Path>, F: AsRef<Path>>(
        workspace_path: P,
        full_path: F,
    ) -> Result<Self, FSError> {
        let fp = full_path.as_ref();
        let relative = fp
            .strip_prefix(workspace_path)
            .map_err(|_e| FSError::InvalidPath {
                path: path_to_string(&full_path),
            })?;
        let path_list = relative
            .components()
            .map(|component| {
                let os_str = component.as_os_str();
                let s = match os_str.to_str() {
                    Some(comp) => comp.to_owned(),
                    None => os_str.to_string_lossy().to_string(),
                };
                NotePathSlice::new(s)
            })
            .collect::<Vec<NotePathSlice>>();

        Ok(Self { slices: path_list })
    }

    pub fn is_note(&self) -> bool {
        match self.slices.last() {
            Some(path_slice) => {
                let last_slice: &Path = Path::new(&path_slice.slice);
                last_slice
                    .extension()
                    .and_then(OsStr::to_str)
                    .map_or_else(|| false, |s| s == "md")
            }
            None => false,
        }
    }

    pub fn get_parent_path(&self) -> (NotePath, String) {
        let mut new_path = self.slices.clone();
        let current = new_path.pop().map_or_else(|| "".to_string(), |s| s.slice);

        (Self { slices: new_path }, current)
    }
}

impl Display for NotePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}{}",
            PATH_SEPARATOR,
            self.slices
                .iter()
                .map(|s| { s.to_string() })
                .collect::<Vec<String>>()
                .join(&PATH_SEPARATOR.to_string())
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct NotePathSlice {
    slice: String,
}

impl NotePathSlice {
    fn new<S: Into<String>>(slice: S) -> Self {
        let re = regex::Regex::new(NON_VALID_PATH_CHARS_REGEX).unwrap();

        let into = slice.into();
        let final_slice = re.replace_all(&into, "_");

        Self {
            slice: final_slice.to_string(),
        }
    }
}

impl Display for NotePathSlice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.slice)
    }
}

fn filter_files(dir: &ignore::DirEntry) -> bool {
    !dir.path().starts_with(".")
}

pub fn get_file_walker<P: AsRef<Path>>(
    base_path: P,
    path: &NotePath,
    recurse: bool,
) -> WalkParallel {
    let w = WalkBuilder::new(path.to_pathbuf(base_path))
        .max_depth(if recurse { None } else { Some(1) })
        .filter_entry(filter_files)
        // .threads(0)
        .build_parallel();

    w
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::core_notes::utilities::path_to_string;

    use super::{NotePath, NotePathSlice};

    #[test]
    fn test_slice_char_replace() {
        let slice_str = "Some?unvalid:chars?";
        let slice = NotePathSlice::new(slice_str);

        assert_eq!("Some_unvalid_chars_", slice.slice);
    }

    #[test]
    fn test_path_create_from_string() {
        let path = "this/is/five/level/path";
        let path = NotePath::from(path);

        assert_eq!(5, path.slices.len());
        assert_eq!("this", path.slices[0].slice);
        assert_eq!("is", path.slices[1].slice);
        assert_eq!("five", path.slices[2].slice);
        assert_eq!("level", path.slices[3].slice);
        assert_eq!("path", path.slices[4].slice);
    }

    #[test]
    fn test_path_with_unvalid_chars() {
        let path = "t*his/i+s/caca?/";
        let path = NotePath::from(path);

        assert_eq!(3, path.slices.len());
        assert_eq!("t_his", path.slices[0].slice);
        assert_eq!("i+s", path.slices[1].slice);
        assert_eq!("caca_", path.slices[2].slice);
    }

    #[test]
    fn test_to_path_buf() {
        let workspace_path = PathBuf::from("/usr/john/notes");
        let path = "/some/subpath";
        let path = NotePath::from(path);
        let path_buf = path.to_pathbuf(&workspace_path);

        let path_string = path_to_string(path_buf);
        assert_eq!("/usr/john/notes/some/subpath", path_string);
    }

    #[test]
    fn test_path_check_valid() {
        let path = PathBuf::from("/some/valid/path/workspace/note.md");
        let workspace = PathBuf::from("/some/valid/path");

        let entry = NotePath::from_path(&workspace, &path).unwrap();

        assert_eq!("/workspace/note.md", entry.to_string());
    }
}
