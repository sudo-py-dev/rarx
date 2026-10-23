use std::io::{Read, Seek};

use crate::stream::StreamCodec;
use crate::tar::{StreamReader, TarReader};
use crate::{Entry, FormatError, FormatId};

/// A seekable byte source an archive is read from.
pub trait ReadSeek: Read + Seek + Send {}

impl<T: Read + Seek + Send> ReadSeek for T {}

/// An opened archive: its directory plus access to each item's data.
pub trait ArchiveReader: Send {
    /// Format of this archive.
    fn format(&self) -> FormatId;

    /// All items, in archive order.
    fn entries(&self) -> &[Entry];

    /// Archive-level comment, if any.
    fn comment(&self) -> Option<&str>;

    /// Opens a stream of the decoded data of item `index`.
    ///
    /// The stream verifies size and checksum as it is read and reports a mismatch as
    /// an [`std::io::ErrorKind::InvalidData`] error carrying a [`crate::DataError`].
    ///
    /// # Errors
    /// [`FormatError`] if the item cannot be located, uses an unsupported method,
    /// or is encrypted.
    fn open_entry(&mut self, index: usize) -> Result<Box<dyn Read + '_>, FormatError>;
}

/// Opens `src` as an archive of the given format. `name_hint` names the single item
/// of a plain compressed stream (e.g. `notes.txt` for `notes.txt.gz`).
///
/// # Errors
/// [`FormatError::Unsupported`] if RarX cannot read that format yet, or any error
/// from parsing the archive's headers.
pub fn open(
    format: FormatId,
    mut src: Box<dyn ReadSeek>,
    name_hint: &str,
) -> Result<Box<dyn ArchiveReader>, FormatError> {
    if let Some(codec) = StreamCodec::from_format(format) {
        return Ok(if crate::tar::starts_with_tar(&mut src, codec)? {
            Box::new(TarReader::new(src, Some(codec))?)
        } else {
            Box::new(StreamReader::new(src, codec, name_hint))
        });
    }
    match format {
        FormatId::Zip => Ok(Box::new(crate::zip::ZipReader::new(src)?)),
        FormatId::SevenZip => Ok(Box::new(crate::sevenz::SevenZReader::new(src)?)),
        FormatId::Tar => Ok(Box::new(TarReader::new(src, None)?)),
        other => Err(FormatError::Unsupported(format!(
            "reading {} archives",
            other.name()
        ))),
    }
}
