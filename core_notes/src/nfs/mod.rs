pub mod visitors;
// Contains the structs to support the data types
use std::{
    ffi::OsStr,
    fmt::Display,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use ignore::{WalkBuilder, WalkParallel};
use serde::{de::Visitor, Deserialize, Serialize};

use super::{error::IOErrors, utilities::path_to_string};

const HASH_SEED: i64 = 0;
const PATH_SEPARATOR: char = '/';
// non valid chars
const NON_VALID_PATH_CHARS_REGEX: &str = r#"[\\/:*?"<>|]"#;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NoteEntry {
    pub path: NotePath,
    pub path_string: String,
    pub data: EntryData,
}

// impl PartialOrd for NoteEntry {
//     fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
//         match self.data.get_ord().partial_cmp(&other.data.get_ord()) {
//             Some(core::cmp::Ordering::Equal) => {}
//             ord => return ord,
//         }
//         // match self.path.to_string().partial_cmp(&other.path.to_string()) {
//         //     Some(core::cmp::Ordering::Equal) => {}
//         //     ord => return ord,
//         // }
//         self.path_string.partial_cmp(&other.path_string)
//     }
// }

// impl Ord for NoteEntry {
//     fn cmp(&self, other: &Self) -> std::cmp::Ordering {
//         if let Some(ord) = self.partial_cmp(other) {
//             ord
//         } else {
//             core::cmp::Ordering::Equal
//         }
//     }
// }

impl AsRef<str> for NoteEntry {
    fn as_ref(&self) -> &str {
        self.path_string.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum EntryData {
    // File size, for fast check
    Note(NoteData),
    Directory(DirectoryData),
    Attachment,
}

impl EntryData {
    fn get_ord(&self) -> u8 {
        match self {
            EntryData::Directory(_directory_data) => 0,
            EntryData::Note(_note_data) => 1,
            EntryData::Attachment => 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NoteData {
    pub path: NotePath,
    pub size: u64,
    pub modified_secs: u64,
}
impl NoteData {
    pub fn get_details<P: AsRef<Path>>(
        &self,
        workspace_path: P,
        path: &NotePath,
    ) -> anyhow::Result<NoteDetails> {
        let content = Some(load_content(&workspace_path, path, true)?);
        let hash = content
            .as_ref()
            .map(|content| gxhash::gxhash32(content.as_bytes(), HASH_SEED));
        Ok(NoteDetails {
            base_path: workspace_path.as_ref().to_path_buf(),
            note_path: path.clone(),
            content,
            hash,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DirectoryData {
    pub path: NotePath,
}
impl DirectoryData {
    pub fn get_details<P: AsRef<Path>>(
        &self,
        workspace_path: P,
    ) -> anyhow::Result<DirectoryDetails> {
        Ok(DirectoryDetails {
            base_path: workspace_path.as_ref().to_path_buf(),
            note_path: self.path.clone(),
        })
    }
}

fn _get_dir_content_size<P: AsRef<Path>>(
    workspace_path: P,
    path: &NotePath,
) -> Result<u64, IOErrors> {
    let os_path = path.into_path(&workspace_path);
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

impl NoteEntry {
    pub fn new<P: AsRef<Path>>(workspace_path: P, path: NotePath) -> Result<Self, IOErrors> {
        let os_path = path.into_path(&workspace_path);
        if !os_path.exists() {
            return Err(IOErrors::NoFileOrDirectoryFound {
                path: path_to_string(os_path),
            });
        }

        let kind = if os_path.is_dir() {
            EntryData::Directory(DirectoryData { path: path.clone() })
        } else if path.is_note() {
            let metadata = os_path.metadata()?;
            let size = metadata.len();
            let modified_secs = metadata
                .modified()
                .map(|t| t.duration_since(UNIX_EPOCH).unwrap().as_secs())
                .unwrap_or_else(|_e| 0);
            EntryData::Note(NoteData {
                path: path.clone(),
                size,
                modified_secs,
            })
        } else {
            EntryData::Attachment
        };
        let path_string = path.to_string();

        Ok(NoteEntry {
            path,
            path_string,
            data: kind,
        })
    }

    pub fn from_path<P: AsRef<Path>, F: AsRef<Path>>(
        workspace_path: P,
        full_path: F,
    ) -> Result<Self, IOErrors> {
        let note_path = NotePath::from_path(&workspace_path, &full_path)?;
        Self::new(&workspace_path, note_path)
    }
}

impl Display for NoteEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.data {
            EntryData::Note(_details) => write!(f, "[NOT] {}", self.path),
            EntryData::Directory(_details) => write!(f, "[DIR] {}", self.path),
            EntryData::Attachment => write!(f, "[ATT]"),
        }
    }
}

#[derive(Debug, Clone)]
pub enum NoteEntryDetails {
    // Hash
    Note(NoteDetails),
    // contents size
    Directory(DirectoryDetails),
    None,
}

impl NoteEntryDetails {
    pub fn get_content(&mut self) -> String {
        match self {
            NoteEntryDetails::Note(note_details) => note_details.get_content(),
            NoteEntryDetails::Directory(_directory_details) => String::new(),
            NoteEntryDetails::None => String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct NoteDetails {
    pub base_path: PathBuf,
    pub note_path: NotePath,
    // Content and hash may be lazy fetched
    hash: Option<u32>,
    content: Option<String>,
}

impl NoteDetails {
    pub fn new(
        base_path: PathBuf,
        note_path: NotePath,
        hash: Option<u32>,
        content: Option<String>,
    ) -> Self {
        Self {
            base_path,
            note_path,
            hash,
            content,
        }
    }

    fn update_content(&mut self) -> (String, u32) {
        let content = load_content(&self.base_path, &self.note_path, true).unwrap_or_default();
        let hash = gxhash::gxhash32(content.as_bytes(), HASH_SEED);
        self.content = Some(content.clone());
        self.hash = Some(hash);
        (content, hash)
    }
    pub fn get_content(&mut self) -> String {
        let content = self.content.clone();
        if let Some(content) = content {
            content
        } else {
            let (content, _) = self.update_content();
            content
        }
    }
    pub fn get_hash(&mut self) -> u32 {
        let hash = self.hash;
        if let Some(hash) = hash {
            hash
        } else {
            let (_, hash) = self.update_content();
            hash
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DirectoryDetails {
    pub base_path: PathBuf,
    pub note_path: NotePath,
}

pub fn load_content<P: AsRef<Path>>(
    workspace_path: P,
    path: &NotePath,
    no_special_chars: bool,
) -> anyhow::Result<String> {
    let os_path = path.into_path(&workspace_path);
    let file = std::fs::read(&os_path)?;
    let mut content = String::from_utf8(file)?;
    if no_special_chars {
        content = super::utilities::remove_diacritics(&content);
    }
    Ok(content)
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

struct NotePathVisitor;
impl<'de> Visitor<'de> for NotePathVisitor {
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
        deserializer.deserialize_str(NotePathVisitor)
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

impl NotePath {
    pub fn new<S: AsRef<str>>(path: S) -> Self {
        let path_list = path
            .as_ref()
            .split(PATH_SEPARATOR)
            .filter(|p| !p.is_empty()) // We remove the empty ones,
            // so `//` are treated as `/`
            .map(NotePathSlice::new)
            .collect();
        Self { slices: path_list }
    }

    pub fn root() -> Self {
        Self::new("")
    }

    pub fn into_path<P: AsRef<Path>>(&self, workspace_path: P) -> PathBuf {
        let mut path = workspace_path.as_ref().to_path_buf();
        for p in &self.slices {
            let slice = p.slice.clone();
            path = path.join(&slice);
        }
        path
    }

    pub fn get_slices(&self) -> Vec<NotePathSlice> {
        self.slices.clone()
    }

    pub fn get_name(&self) -> String {
        self.slices
            .last()
            .map_or_else(String::new, |s| s.slice.clone())
    }

    pub fn from_path<P: AsRef<Path>, F: AsRef<Path>>(
        workspace_path: P,
        full_path: F,
    ) -> Result<Self, IOErrors> {
        let fp = full_path.as_ref();
        let relative = fp
            .strip_prefix(workspace_path)
            .map_err(|_e| IOErrors::InvalidPath {
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
pub struct NotePathSlice {
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
    let w = WalkBuilder::new(path.into_path(base_path))
        .max_depth(if recurse { None } else { Some(1) })
        .filter_entry(filter_files)
        // .threads(0)
        .build_parallel();

    w
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::utilities::path_to_string;

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
        let path = NotePath::new(path);

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
        let path = NotePath::new(path);

        assert_eq!(3, path.slices.len());
        assert_eq!("t_his", path.slices[0].slice);
        assert_eq!("i+s", path.slices[1].slice);
        assert_eq!("caca_", path.slices[2].slice);
    }

    #[test]
    fn test_to_path_buf() {
        let workspace_path = PathBuf::from("/usr/john/notes");
        let path = "/some/subpath";
        let path = NotePath::new(path);
        let path_buf = path.into_path(&workspace_path);

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