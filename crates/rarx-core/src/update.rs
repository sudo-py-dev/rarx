//! Creating and modifying archives: add, update, freshen, synchronise, delete
//! (AGENT.md §10.1–§10.2, §10.6).
//!
//! The new archive is always written to a temporary file next to the target and
//! renamed over it only after it is complete, so a failure or cancellation never
//! damages an existing archive.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File, Metadata};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Component, Path, PathBuf};

use rarx_formats::sevenz::{SevenZOptions, SevenZReader, SevenZWriter};
use rarx_formats::zip::{ZipMethod, ZipReader, ZipWriter};
use rarx_formats::{ArchiveReader, Entry, EntryKind, FileTime, FormatError, FormatId, NewItem};

use crate::select::Selection;
use crate::time::{from_system_time, to_local_civil, to_system_time};
use crate::{Error, JobControl, Result};

/// How names of added files are formed from the paths given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StorePaths {
    /// Keep the path as typed, minus any root, drive or `..` (WinRAR default).
    AsGiven,
    /// Start names at the last component of each path given (7-Zip default, `-ep1`).
    Relative,
    /// Store file names only (`-ep`).
    None,
}

/// Which disk files go into the archive and what happens to existing items.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateMode {
    /// Add every file, replacing items with the same name (`a`).
    Add,
    /// Add new files and replace items only with newer files (`u`).
    Update,
    /// Only replace existing items with newer files (`f`).
    Freshen,
    /// Like [`UpdateMode::Update`], and delete items whose file no longer exists.
    Synchronize,
}

/// Settings for [`add`].
#[derive(Clone, Debug)]
#[allow(clippy::struct_excessive_bools)] // one flag per command-line switch
pub struct AddOptions {
    /// Compression level: 0 stores, 1–9 trade speed for size.
    pub level: u32,
    /// Naming of added files.
    pub paths: StorePaths,
    /// Policy for existing items.
    pub mode: UpdateMode,
    /// Masks of names to leave out (only exclude masks are used).
    pub exclude: Selection,
    /// Store symbolic links as links instead of the files they point to.
    pub store_symlinks: bool,
    /// Delete source files once they are safely in the archive (`-sdel`, `-df`, `m`).
    pub delete_sources: bool,
    /// 7z: one solid block for all files instead of one block per file (`-ms`).
    pub solid: bool,
    /// 7z: compress the header database (`-mhc`).
    pub compress_header: bool,
    /// Always write ZIP64 records (testing aid).
    pub force_zip64: bool,
}

impl Default for AddOptions {
    fn default() -> Self {
        Self {
            level: 5,
            paths: StorePaths::Relative,
            mode: UpdateMode::Add,
            exclude: Selection::all(),
            store_symlinks: false,
            delete_sources: false,
            solid: true,
            compress_header: true,
            force_zip64: false,
        }
    }
}

/// What happened to one archive item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemAction {
    /// A new item was added from disk.
    Added,
    /// An existing item was replaced from disk.
    Replaced,
    /// An existing item was copied over unchanged.
    Kept,
    /// An existing item was removed.
    Deleted,
}

/// Receives progress during [`add`] and [`delete`].
pub trait UpdateHandler {
    /// Called after each archive item is written or dropped.
    fn item_done(&mut self, _name: &str, _action: ItemAction) {}

    /// Called for a source that could not be read; it is skipped.
    fn warning(&mut self, _path: &Path, _message: &str) {}
}

/// An [`UpdateHandler`] that ignores everything.
#[derive(Debug, Default)]
pub struct SilentUpdate;

impl UpdateHandler for SilentUpdate {}

/// Totals for [`add`] or [`delete`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UpdateReport {
    /// Items added from disk.
    pub added: u64,
    /// Items replaced from disk.
    pub replaced: u64,
    /// Items copied unchanged from the old archive.
    pub kept: u64,
    /// Items removed.
    pub deleted: u64,
    /// Uncompressed bytes read from disk.
    pub bytes: u64,
    /// Sources that could not be read, with the reason.
    pub warnings: Vec<(PathBuf, String)>,
    /// Whether the archive file was written (false when there was nothing to do).
    pub written: bool,
}

/// A file-system object selected for archiving.
#[derive(Debug)]
struct Source {
    path: PathBuf,
    name: String,
    kind: EntryKind,
    meta: Metadata,
}

/// Adds files and folders to the archive at `archive_path`, creating it if needed.
///
/// # Errors
/// [`Error::Unsupported`] for archive types RarX cannot write, [`Error::Cancelled`],
/// or any I/O or format error; the original archive is left untouched on error.
pub fn add(
    archive_path: &Path,
    inputs: &[PathBuf],
    options: &AddOptions,
    handler: &mut dyn UpdateHandler,
    ctl: &JobControl,
) -> Result<UpdateReport> {
    let kind = target_kind(archive_path)?;
    let mut report = UpdateReport::default();
    let skip = [absolute(archive_path)];
    let sources = collect_sources(inputs, options, &skip, handler, &mut report);
    let mut existing = OldArchive::open(archive_path, kind)?;

    let replaces_old = |source: &Source, old: &Entry| match options.mode {
        UpdateMode::Add => true,
        _ => source.kind == EntryKind::File && is_newer(&source.meta, old),
    };
    let nothing_new = sources.is_empty() && options.mode != UpdateMode::Synchronize;
    if nothing_new || (existing.is_none() && options.mode == UpdateMode::Freshen) {
        return Ok(report);
    }

    let by_name: HashMap<&str, usize> = sources
        .iter()
        .enumerate()
        .map(|(i, s)| (s.name.as_str(), i))
        .collect();
    let mut used = vec![false; sources.len()];
    let mut archived = Vec::new();

    let report = write_atomically(archive_path, kind, options, |writer| {
        if let Some(reader) = existing.as_mut() {
            let entries = reader.entries().to_vec();
            for (index, old) in entries.iter().enumerate() {
                ctl.checkpoint()?;
                match by_name.get(old.name.as_str()) {
                    Some(&i) if replaces_old(&sources[i], old) => {
                        used[i] = true;
                        if write_source(writer, &sources[i], options, &mut report, handler)? {
                            archived.push(i);
                            report.replaced += 1;
                            handler.item_done(&old.name, ItemAction::Replaced);
                        }
                    }
                    Some(&i) => {
                        used[i] = true;
                        archived.push(i);
                        keep(reader, index, writer, &mut report, handler)?;
                    }
                    None if options.mode == UpdateMode::Synchronize => {
                        report.deleted += 1;
                        handler.item_done(&old.name, ItemAction::Deleted);
                    }
                    None => keep(reader, index, writer, &mut report, handler)?,
                }
            }
        }
        if options.mode != UpdateMode::Freshen {
            for (i, source) in sources.iter().enumerate().filter(|(i, _)| !used[*i]) {
                ctl.checkpoint()?;
                if write_source(writer, source, options, &mut report, handler)? {
                    archived.push(i);
                    report.added += 1;
                    handler.item_done(&source.name, ItemAction::Added);
                }
            }
        }
        Ok(report)
    })?;

    if options.delete_sources {
        delete_sources(&sources, &archived, handler);
    }
    Ok(report)
}

/// Removes the items matching `selection` from the archive at `archive_path`.
///
/// # Errors
/// [`Error::Unsupported`] for archive types RarX cannot write, [`Error::Cancelled`],
/// or any I/O or format error; the original archive is left untouched on error.
pub fn delete(
    archive_path: &Path,
    selection: &Selection,
    handler: &mut dyn UpdateHandler,
    ctl: &JobControl,
) -> Result<UpdateReport> {
    let kind = target_kind(archive_path)?;
    let Some(mut reader) = OldArchive::open(archive_path, kind)? else {
        return Err(Error::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("{} does not exist", archive_path.display()),
        )));
    };
    let entries = reader.entries().to_vec();
    if !entries.iter().any(|e| selection.matches(&e.name)) {
        return Ok(UpdateReport::default());
    }
    let mut report = UpdateReport::default();
    let options = reader.rewrite_options();
    write_atomically(archive_path, kind, &options, |writer| {
        for (index, entry) in entries.iter().enumerate() {
            ctl.checkpoint()?;
            if selection.matches(&entry.name) {
                report.deleted += 1;
                handler.item_done(&entry.name, ItemAction::Deleted);
            } else {
                keep(&mut reader, index, writer, &mut report, handler)?;
            }
        }
        Ok(report)
    })
}

/// Archive formats RarX can write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Zip,
    SevenZip,
}

/// The archive being updated.
enum OldArchive {
    Zip(ZipReader),
    SevenZip(SevenZReader),
}

impl OldArchive {
    fn open(path: &Path, kind: Kind) -> Result<Option<Self>> {
        if !path.exists() {
            return Ok(None);
        }
        let src = Box::new(BufReader::new(File::open(path)?));
        Ok(Some(match kind {
            Kind::Zip => Self::Zip(ZipReader::new(src)?),
            Kind::SevenZip => Self::SevenZip(SevenZReader::new(src)?),
        }))
    }

    fn entries(&self) -> &[Entry] {
        match self {
            Self::Zip(r) => r.entries(),
            Self::SevenZip(r) => r.entries(),
        }
    }

    /// Settings used when an archive is rewritten without new files (delete).
    fn rewrite_options(&self) -> AddOptions {
        let solid = match self {
            Self::Zip(_) => true,
            Self::SevenZip(r) => r.is_solid(),
        };
        AddOptions {
            solid,
            ..AddOptions::default()
        }
    }
}

/// The archive being written.
enum NewArchive {
    Zip(ZipWriter<BufWriter<File>>),
    SevenZip(SevenZWriter<BufWriter<File>>),
}

impl NewArchive {
    fn add_directory(&mut self, item: &NewItem) -> std::io::Result<()> {
        match self {
            Self::Zip(w) => w.add_directory(item),
            Self::SevenZip(w) => w.add_directory(item),
        }
    }

    fn add_symlink(&mut self, item: &NewItem, target: &str) -> std::io::Result<()> {
        match self {
            Self::Zip(w) => w.add_symlink(item, target),
            Self::SevenZip(w) => w.add_symlink(item, target),
        }
    }

    fn add_file(&mut self, item: &NewItem, level: u32, data: &mut dyn Read) -> std::io::Result<()> {
        match self {
            Self::Zip(w) => {
                let method = match level {
                    0 => ZipMethod::Store,
                    level => ZipMethod::Deflate {
                        level: level.min(9),
                    },
                };
                w.add_file(item, method, data)
            }
            Self::SevenZip(w) => w.add_file(item, data),
        }
    }

    fn finish(self) -> std::io::Result<BufWriter<File>> {
        match self {
            Self::Zip(w) => w.finish(None),
            Self::SevenZip(w) => w.finish(),
        }
    }
}

/// Writes a new archive into a temporary file and renames it over `target`.
fn write_atomically(
    target: &Path,
    kind: Kind,
    options: &AddOptions,
    body: impl FnOnce(&mut NewArchive) -> Result<UpdateReport>,
) -> Result<UpdateReport> {
    let temp = temp_path(target);
    let result = (|| {
        let file = File::options().write(true).create_new(true).open(&temp)?;
        let out = BufWriter::new(file);
        let mut writer = match kind {
            Kind::Zip => {
                let mut zip = ZipWriter::new(out);
                zip.set_force_zip64(options.force_zip64);
                NewArchive::Zip(zip)
            }
            Kind::SevenZip => {
                let settings = SevenZOptions {
                    level: options.level,
                    solid: options.solid,
                    compress_header: options.compress_header,
                };
                NewArchive::SevenZip(SevenZWriter::new(out, settings)?)
            }
        };
        let mut report = body(&mut writer)?;
        let mut out = writer.finish()?;
        out.flush()?;
        out.into_inner()
            .map_err(std::io::IntoInnerError::into_error)?
            .sync_all()?;
        fs::rename(&temp, target)?;
        report.written = true;
        Ok(report)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn temp_path(target: &Path) -> PathBuf {
    let name = target.file_name().unwrap_or_default().to_string_lossy();
    target.with_file_name(format!(".{name}.rarx-{}.tmp", std::process::id()))
}

/// Which format to write: that of the existing archive, else from the extension.
fn target_kind(archive_path: &Path) -> Result<Kind> {
    if archive_path.exists() {
        return match crate::detect_file(archive_path)? {
            Some(FormatId::Zip) => Ok(Kind::Zip),
            Some(FormatId::SevenZip) => Ok(Kind::SevenZip),
            Some(other) => Err(Error::Unsupported(format!(
                "updating {} archives",
                other.name()
            ))),
            None => Err(Error::UnknownFormat(archive_path.to_owned())),
        };
    }
    let ext = archive_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default();
    match ext.to_ascii_lowercase().as_str() {
        "zip" | "jar" | "zipx" | "apk" | "epub" | "xpi" | "cbz" => Ok(Kind::Zip),
        "7z" => Ok(Kind::SevenZip),
        "" => Err(Error::Unsupported(
            "archives without an extension; add .zip or .7z".to_owned(),
        )),
        other => Err(Error::Unsupported(format!("creating .{other} archives"))),
    }
}

/// Carries an existing item over into the new archive: ZIP items are copied byte
/// for byte; 7z items are decoded and re-encoded, because they share solid blocks.
fn keep(
    reader: &mut OldArchive,
    index: usize,
    writer: &mut NewArchive,
    report: &mut UpdateReport,
    handler: &mut dyn UpdateHandler,
) -> Result<()> {
    let entry = reader.entries()[index].clone();
    match (reader, writer) {
        (OldArchive::Zip(r), NewArchive::Zip(w)) => r.copy_raw(index, w)?,
        (OldArchive::SevenZip(r), writer) => {
            let mut item = NewItem::new(entry.name.clone(), entry.kind);
            item.modified = entry.modified;
            item.unix_mode = entry.unix_mode.map(|m| m & 0o7777);
            item.dos_attributes = entry.dos_attributes;
            item.size_hint = entry.size;
            match entry.kind {
                EntryKind::Directory => writer.add_directory(&item)?,
                EntryKind::Symlink => {
                    let mut target = String::new();
                    r.open_entry(index)?.read_to_string(&mut target)?;
                    writer.add_symlink(&item, &target)?;
                }
                EntryKind::File => writer.add_file(&item, 0, &mut r.open_entry(index)?)?,
            }
        }
        (OldArchive::Zip(_), NewArchive::SevenZip(_)) => {
            return Err(Error::Unsupported("converting ZIP to 7z".to_owned()));
        }
    }
    report.kept += 1;
    handler.item_done(&entry.name, ItemAction::Kept);
    Ok(())
}

/// Writes one disk source. Returns `false` (with a warning) if it could not be read.
fn write_source(
    writer: &mut NewArchive,
    source: &Source,
    options: &AddOptions,
    report: &mut UpdateReport,
    handler: &mut dyn UpdateHandler,
) -> Result<bool> {
    let modified = source.meta.modified().ok();
    let mut item = NewItem::new(source.name.clone(), source.kind);
    item.modified = modified.and_then(from_system_time);
    item.modified_local = modified.and_then(to_local_civil);
    item.unix_mode = unix_mode(&source.meta);
    item.dos_attributes = dos_attributes(&source.meta);
    item.size_hint = source.meta.len();

    let mut warn = |message: String, report: &mut UpdateReport| {
        handler.warning(&source.path, &message);
        report.warnings.push((source.path.clone(), message));
        Ok(false)
    };
    match source.kind {
        EntryKind::Directory => writer.add_directory(&item)?,
        EntryKind::Symlink => match fs::read_link(&source.path) {
            Ok(target) => writer.add_symlink(&item, &target.to_string_lossy())?,
            Err(err) => return warn(err.to_string(), report),
        },
        EntryKind::File => {
            let file = match File::open(&source.path) {
                Ok(file) => file,
                Err(err) => return warn(err.to_string(), report),
            };
            let mut counted = CountingReader {
                inner: BufReader::new(file),
                count: 0,
            };
            writer
                .add_file(&item, options.level, &mut counted)
                .map_err(|err| {
                    Error::Format(FormatError::Io(std::io::Error::new(
                        err.kind(),
                        format!("{}: {err}", source.path.display()),
                    )))
                })?;
            report.bytes += counted.count;
        }
    }
    Ok(true)
}

struct CountingReader<R> {
    inner: R,
    count: u64,
}

impl<R: std::io::Read> std::io::Read for CountingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.count += n as u64;
        Ok(n)
    }
}

/// Whether the disk file is newer than the archived item, in whole seconds. DOS
/// timestamps round down to even seconds, so one extra second is tolerated.
fn is_newer(meta: &Metadata, old: &Entry) -> bool {
    let seconds = |time: Option<FileTime>| match time {
        Some(FileTime::Unix { seconds, .. }) => Some(seconds),
        _ => None,
    };
    let disk = seconds(meta.modified().ok().and_then(from_system_time));
    let archived = seconds(
        old.modified
            .and_then(to_system_time)
            .and_then(from_system_time),
    );
    let slack = i64::from(matches!(old.modified, Some(FileTime::Local(_))));
    match (disk, archived) {
        (Some(disk), Some(archived)) => disk > archived + slack,
        _ => true,
    }
}

#[cfg(unix)]
#[allow(clippy::unnecessary_wraps)] // `None` on other platforms
fn unix_mode(meta: &Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    Some(meta.permissions().mode() & 0o7777)
}

#[cfg(not(unix))]
fn unix_mode(_meta: &Metadata) -> Option<u32> {
    None
}

#[cfg(windows)]
fn dos_attributes(meta: &Metadata) -> Option<u32> {
    use std::os::windows::fs::MetadataExt;
    Some(meta.file_attributes() & 0xFFFF)
}

#[cfg(not(windows))]
#[allow(clippy::unnecessary_wraps)] // `None` on other platforms
fn dos_attributes(meta: &Metadata) -> Option<u32> {
    let readonly = u32::from(meta.permissions().readonly());
    Some(if meta.is_dir() { 0x10 } else { 0x20 } | readonly)
}

/// Walks the inputs and returns everything to archive, in a stable order.
fn collect_sources(
    inputs: &[PathBuf],
    options: &AddOptions,
    skip: &[Option<PathBuf>],
    handler: &mut dyn UpdateHandler,
    report: &mut UpdateReport,
) -> Vec<Source> {
    let mut walker = Walker {
        options,
        skip,
        visited: HashSet::new(),
        sources: Vec::new(),
        names: HashMap::new(),
        handler,
        report,
    };
    for input in inputs {
        let prefix = match options.paths {
            StorePaths::AsGiven => normal_components(input),
            StorePaths::Relative | StorePaths::None => input
                .file_name()
                .filter(|_| !matches!(input.components().next_back(), Some(Component::ParentDir)))
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        };
        walker.visit(input, &prefix);
    }
    walker.sources
}

struct Walker<'a> {
    options: &'a AddOptions,
    skip: &'a [Option<PathBuf>],
    visited: HashSet<PathBuf>,
    sources: Vec<Source>,
    /// Index into `sources` for each name, so a later duplicate replaces it.
    names: HashMap<String, usize>,
    handler: &'a mut dyn UpdateHandler,
    report: &'a mut UpdateReport,
}

impl Walker<'_> {
    fn warn(&mut self, path: &Path, message: String) {
        self.handler.warning(path, &message);
        self.report.warnings.push((path.to_owned(), message));
    }

    fn visit(&mut self, path: &Path, name: &str) {
        let meta = if self.options.store_symlinks {
            fs::symlink_metadata(path)
        } else {
            fs::metadata(path)
        };
        let meta = match meta {
            Ok(meta) => meta,
            Err(err) => return self.warn(path, err.to_string()),
        };
        if !name.is_empty() && !self.options.exclude.matches(name) {
            return;
        }
        if self
            .skip
            .iter()
            .flatten()
            .any(|s| Some(s) == absolute(path).as_ref())
        {
            return;
        }
        let flat = self.options.paths == StorePaths::None;
        if meta.is_symlink() {
            self.push(path, name, EntryKind::Symlink, meta, flat);
        } else if meta.is_dir() {
            // Following links can revisit a folder; stop at the second visit.
            if let Ok(canonical) = fs::canonicalize(path)
                && !self.visited.insert(canonical)
            {
                return;
            }
            if !name.is_empty() && !flat {
                self.push(path, name, EntryKind::Directory, meta, flat);
            }
            let mut children = match fs::read_dir(path) {
                Ok(rd) => rd
                    .filter_map(std::result::Result::ok)
                    .map(|e| e.path())
                    .collect::<Vec<_>>(),
                Err(err) => return self.warn(path, err.to_string()),
            };
            children.sort();
            for child in children {
                let child_name = child
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                let full = if name.is_empty() {
                    child_name
                } else {
                    format!("{name}/{child_name}")
                };
                self.visit(&child, &full);
            }
        } else {
            self.push(path, name, EntryKind::File, meta, flat);
        }
    }

    fn push(&mut self, path: &Path, name: &str, kind: EntryKind, meta: Metadata, flat: bool) {
        let name = if flat {
            name.rsplit('/').next().unwrap_or(name)
        } else {
            name
        };
        if name.is_empty() {
            return;
        }
        let source = Source {
            path: path.to_owned(),
            name: name.to_owned(),
            kind,
            meta,
        };
        if let Some(&i) = self.names.get(name) {
            self.sources[i] = source;
        } else {
            self.names.insert(name.to_owned(), self.sources.len());
            self.sources.push(source);
        }
    }
}

/// Joins the normal components of `path` with `/`, dropping roots, drives, `.`
/// and `..`.
fn normal_components(path: &Path) -> String {
    path.components()
        .filter_map(|c| match c {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn absolute(path: &Path) -> Option<PathBuf> {
    fs::canonicalize(path).ok().or_else(|| {
        let parent = fs::canonicalize(path.parent().filter(|p| !p.as_os_str().is_empty())?).ok()?;
        Some(parent.join(path.file_name()?))
    })
}

/// Deletes archived sources: files first, then folders deepest first (only if empty).
fn delete_sources(sources: &[Source], archived: &[usize], handler: &mut dyn UpdateHandler) {
    let mut folders = Vec::new();
    for &i in archived {
        let source = &sources[i];
        match source.kind {
            EntryKind::Directory => folders.push(&source.path),
            EntryKind::File | EntryKind::Symlink => {
                if let Err(err) = fs::remove_file(&source.path) {
                    handler.warning(&source.path, &format!("cannot delete: {err}"));
                }
            }
        }
    }
    folders.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
    for folder in folders {
        let _ = fs::remove_dir(folder);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_given_names_drop_unsafe_parts() {
        assert_eq!(
            normal_components(Path::new("/abs/dir/file")),
            "abs/dir/file"
        );
        assert_eq!(normal_components(Path::new("../x/./y")), "x/y");
    }

    #[test]
    fn temp_path_is_hidden_sibling() {
        let temp = temp_path(Path::new("/a/b/archive.zip"));
        assert_eq!(temp.parent(), Some(Path::new("/a/b")));
        assert!(
            temp.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".archive.zip.rarx-")
        );
    }
}
