use crate::{CivilTime, EntryKind, FileTime};

/// Metadata for an item being added.
#[derive(Clone, Debug)]
pub struct NewItem {
    /// `/`-separated path inside the archive, without a trailing slash.
    pub name: String,
    /// File, directory or symbolic link.
    pub kind: EntryKind,
    /// Modification time as an absolute time (stored in the UT extra field).
    pub modified: Option<FileTime>,
    /// Modification time in local wall-clock time (stored in the DOS fields).
    pub modified_local: Option<CivilTime>,
    /// Unix permission bits; when set, the item is marked as made on Unix.
    pub unix_mode: Option<u32>,
    /// MS-DOS / Windows attribute bits.
    pub dos_attributes: Option<u32>,
    /// Expected uncompressed size, used to decide on ZIP64 fields up front.
    pub size_hint: u64,
}

impl NewItem {
    /// Creates item metadata with only a name and kind.
    #[must_use]
    pub fn new(name: impl Into<String>, kind: EntryKind) -> Self {
        Self {
            name: name.into(),
            kind,
            modified: None,
            modified_local: None,
            unix_mode: None,
            dos_attributes: None,
            size_hint: 0,
        }
    }
}
