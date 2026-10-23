use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use rarx_formats::{ArchiveReader, Entry, FormatError, FormatId};

use crate::{Error, Result};

/// Bytes read from the start of a file for signature detection.
const DETECT_PREFIX_LEN: u64 = 64;

/// Extensions that are ZIP containers even when the file does not start with a ZIP
/// signature (e.g. self-extracting archives).
const ZIP_EXTENSIONS: &[&str] = &[
    "zip", "zipx", "jar", "war", "ear", "apk", "aab", "xpi", "epub", "odt", "ods", "odp", "docx",
    "xlsx", "pptx", "vsix", "nupkg", "whl", "ipa", "exe",
];

/// Identifies the archive format of the file at `path` by its signature.
///
/// # Errors
/// Returns [`Error::Io`] if the file cannot be opened or read.
pub fn detect_file(path: &Path) -> Result<Option<FormatId>> {
    let mut prefix = Vec::new();
    File::open(path)?
        .take(DETECT_PREFIX_LEN)
        .read_to_end(&mut prefix)?;
    Ok(rarx_formats::detect(&prefix))
}

/// An archive opened from disk.
pub struct Archive {
    path: PathBuf,
    reader: Box<dyn ArchiveReader>,
}

impl Archive {
    /// Opens the archive at `path`, detecting its format by signature and then by
    /// extension.
    ///
    /// # Errors
    /// [`Error::UnknownFormat`] if no reader accepts the file, [`Error::Io`] if it
    /// cannot be read, or [`Error::Format`] if its headers are invalid.
    pub fn open(path: &Path) -> Result<Self> {
        let format = match detect_file(path)? {
            Some(format) => format,
            None if has_zip_extension(path) => FormatId::Zip,
            None => return Err(Error::UnknownFormat(path.to_owned())),
        };
        let src = Box::new(BufReader::new(File::open(path)?));
        let reader = rarx_formats::open(format, src).map_err(|err| match err {
            FormatError::Corrupt(_) if format == FormatId::Zip && !starts_like_zip(path) => {
                Error::UnknownFormat(path.to_owned())
            }
            other => Error::Format(other),
        })?;
        Ok(Self {
            path: path.to_owned(),
            reader,
        })
    }

    /// Path the archive was opened from.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Format of the archive.
    #[must_use]
    pub fn format(&self) -> FormatId {
        self.reader.format()
    }

    /// All items, in archive order.
    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        self.reader.entries()
    }

    /// Archive comment, if any.
    #[must_use]
    pub fn comment(&self) -> Option<&str> {
        self.reader.comment()
    }

    /// Opens a verified stream of the decoded data of item `index`.
    ///
    /// # Errors
    /// [`Error::Format`] if the item cannot be decoded (unsupported method,
    /// encryption, damaged headers).
    pub fn read_entry(&mut self, index: usize) -> Result<Box<dyn Read + '_>> {
        Ok(self.reader.open_entry(index)?)
    }

    pub(crate) fn reader_mut(&mut self) -> &mut dyn ArchiveReader {
        self.reader.as_mut()
    }
}

fn has_zip_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ZIP_EXTENSIONS.iter().any(|z| z.eq_ignore_ascii_case(ext)))
}

/// Whether the file begins with any ZIP record signature.
fn starts_like_zip(path: &Path) -> bool {
    matches!(detect_file(path), Ok(Some(FormatId::Zip)))
}
