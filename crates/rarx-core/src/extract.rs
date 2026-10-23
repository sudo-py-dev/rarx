//! Extracting and testing archive items (AGENT.md §10.3–§10.4, §18).

use std::collections::HashSet;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use rarx_formats::{DataError, Entry, EntryKind, FormatError};

use crate::sanitize::{NameRules, sanitize_path};
use crate::select::Selection;
use crate::time::to_system_time;
use crate::{Archive, Error, JobControl, Result};

/// Longest symbolic-link target RarX will create.
const MAX_LINK_TARGET: u64 = 4096;
const COPY_BUFFER: usize = 256 * 1024;

/// How item paths map onto the destination folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathMode {
    /// Recreate the archive's folder structure (`x`).
    Full,
    /// Put every file directly in the destination (`e`).
    None,
}

/// What to do when a file being extracted already exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverwriteMode {
    /// Ask the [`ExtractHandler`] for every conflict.
    Ask,
    /// Replace existing files.
    Overwrite,
    /// Keep existing files and skip the item.
    Skip,
    /// Extract under a new name such as `name_1.ext`.
    RenameNew,
    /// Rename the existing file to a new name, then extract.
    RenameExisting,
}

/// Answer to an overwrite question.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverwriteChoice {
    /// Replace this file.
    Yes,
    /// Keep this file.
    No,
    /// Extract this item under a new name such as `name_1.ext`.
    Rename,
    /// Replace this and every later conflicting file.
    YesToAll,
    /// Keep this and every later conflicting file.
    NoToAll,
    /// Extract this and every later conflicting item under a new name.
    AutoRenameAll,
    /// Stop the whole operation.
    Cancel,
}

/// Extraction settings.
#[derive(Clone, Debug)]
pub struct ExtractOptions {
    /// Path handling.
    pub paths: PathMode,
    /// Conflict handling.
    pub overwrite: OverwriteMode,
    /// Which items to extract.
    pub selection: Selection,
    /// Keep partially extracted files when their data is damaged (`-kb`).
    pub keep_broken: bool,
    /// File-name rules for the destination file system.
    pub name_rules: NameRules,
}

impl Default for ExtractOptions {
    fn default() -> Self {
        Self {
            paths: PathMode::Full,
            overwrite: OverwriteMode::Ask,
            selection: Selection::all(),
            keep_broken: false,
            name_rules: NameRules::native(),
        }
    }
}

/// Why a single item failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemError {
    /// The data's checksum did not match.
    Checksum,
    /// The data is damaged in another way.
    Data(String),
    /// The item uses a method or feature RarX cannot handle yet.
    Unsupported(String),
    /// The item is encrypted and no password was available.
    Encrypted,
    /// The item's name has no safe on-disk equivalent, or a folder on its path is a
    /// symbolic link.
    UnsafePath,
    /// A symbolic link would point outside the destination folder.
    UnsafeLink(String),
    /// Writing to disk failed.
    Io(String),
}

impl fmt::Display for ItemError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Checksum => f.write_str("CRC failed"),
            Self::Data(what) => write!(f, "data error: {what}"),
            Self::Unsupported(what) => write!(f, "unsupported {what}"),
            Self::Encrypted => f.write_str("encrypted (passwords are not supported yet)"),
            Self::UnsafePath => f.write_str("unsafe path, skipped"),
            Self::UnsafeLink(target) => write!(f, "dangerous link target `{target}`, skipped"),
            Self::Io(what) => f.write_str(what),
        }
    }
}

/// Result of processing one item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Extracted or tested successfully.
    Ok,
    /// Left alone because of the overwrite policy.
    Skipped,
    /// Failed; the operation continued with the next item.
    Failed(ItemError),
}

/// Receives progress and answers questions during extraction or testing.
pub trait ExtractHandler {
    /// Called before an item is processed.
    fn item_started(&mut self, _entry: &Entry) {}

    /// Called after an item is processed.
    fn item_done(&mut self, _entry: &Entry, _outcome: &Outcome) {}

    /// Called for an existing file when the mode is [`OverwriteMode::Ask`].
    fn ask_overwrite(&mut self, _path: &Path, _entry: &Entry) -> OverwriteChoice {
        OverwriteChoice::No
    }
}

/// A handler that ignores progress and never overwrites when asked.
#[derive(Debug, Default)]
pub struct SilentHandler;

impl ExtractHandler for SilentHandler {}

/// Totals for a finished operation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Files written or tested.
    pub files: u64,
    /// Folders created.
    pub folders: u64,
    /// Symbolic links created.
    pub links: u64,
    /// Uncompressed bytes written or tested.
    pub bytes: u64,
    /// Items skipped because of the overwrite policy.
    pub skipped: u64,
    /// Items that failed, with the reason.
    pub errors: Vec<(String, ItemError)>,
}

impl Report {
    fn record(&mut self, entry: &Entry, outcome: &Outcome, handler: &mut dyn ExtractHandler) {
        match outcome {
            Outcome::Ok => match entry.kind {
                EntryKind::File => self.files += 1,
                EntryKind::Directory => self.folders += 1,
                EntryKind::Symlink => self.links += 1,
            },
            Outcome::Skipped => self.skipped += 1,
            Outcome::Failed(err) => self.errors.push((entry.name.clone(), err.clone())),
        }
        handler.item_done(entry, outcome);
    }
}

/// Extracts the selected items of `archive` into `dest`.
///
/// Item failures are collected in the [`Report`]; the operation continues with the
/// next item. Symbolic links are created last and only when they stay inside
/// `dest`, and nothing is ever written through an existing link.
///
/// # Errors
/// [`Error::Cancelled`] if `ctl` is cancelled or the handler answers
/// [`OverwriteChoice::Cancel`]; [`Error::Io`] if `dest` cannot be created.
pub fn extract(
    archive: &mut Archive,
    dest: &Path,
    options: &ExtractOptions,
    handler: &mut dyn ExtractHandler,
    ctl: &JobControl,
) -> Result<Report> {
    fs::create_dir_all(dest)?;
    let mut job = ExtractJob {
        archive,
        dest,
        options,
        overwrite: options.overwrite,
        handler,
        ctl,
        report: Report::default(),
        link_paths: HashSet::new(),
    };
    let entries = job.archive.entries().to_vec();
    let mut links = Vec::new();
    let mut folders = Vec::new();

    for (index, entry) in entries.iter().enumerate() {
        ctl.checkpoint()?;
        if !options.selection.matches(&entry.name) {
            continue;
        }
        if options.paths == PathMode::None && entry.is_dir() {
            continue;
        }
        let name = match options.paths {
            PathMode::Full => entry.name.as_str(),
            PathMode::None => entry.name.rsplit(['/', '\\']).next().unwrap_or_default(),
        };
        job.handler.item_started(entry);
        let Some(rel) = sanitize_path(name, options.name_rules) else {
            // A folder named `.` or `./` is the destination itself (GNU tar `-C dir .`).
            if !(entry.is_dir() && is_current_dir(name)) {
                job.finish(entry, &Outcome::Failed(ItemError::UnsafePath));
            }
            continue;
        };
        let outcome = match entry.kind {
            EntryKind::Directory => {
                let outcome = job.make_folder(&rel);
                folders.push((rel, entry));
                outcome
            }
            EntryKind::File => job.write_file(index, entry, &rel)?,
            EntryKind::Symlink => {
                links.push((index, entry, rel));
                continue;
            }
        };
        job.finish(entry, &outcome);
    }

    job.link_paths = links.iter().map(|(_, _, rel)| rel.clone()).collect();
    for (index, entry, rel) in links {
        ctl.checkpoint()?;
        let outcome = job.make_link(index, &rel)?;
        job.finish(entry, &outcome);
    }

    // Deepest folders first so setting a parent's time is not undone by its children.
    folders.sort_by_key(|(rel, _)| std::cmp::Reverse(rel.components().count()));
    for (rel, entry) in folders {
        if let Some(time) = entry.modified.and_then(to_system_time) {
            let _ = File::open(dest.join(rel)).and_then(|dir| dir.set_modified(time));
        }
    }
    Ok(job.report)
}

/// Decodes the selected items of `archive` and verifies their checksums without
/// writing anything.
///
/// # Errors
/// [`Error::Cancelled`] if `ctl` is cancelled.
pub fn test(
    archive: &mut Archive,
    selection: &Selection,
    handler: &mut dyn ExtractHandler,
    ctl: &JobControl,
) -> Result<Report> {
    let mut report = Report::default();
    let entries = archive.entries().to_vec();
    for (index, entry) in entries.iter().enumerate() {
        ctl.checkpoint()?;
        if !selection.matches(&entry.name) {
            continue;
        }
        handler.item_started(entry);
        let outcome = if entry.is_dir() {
            Outcome::Ok
        } else {
            match archive.reader_mut().open_entry(index) {
                Ok(stream) => match copy_cancellable(stream, &mut io::sink(), ctl) {
                    Ok(bytes) => {
                        report.bytes += bytes;
                        Outcome::Ok
                    }
                    Err(CopyError::Cancelled) => return Err(Error::Cancelled),
                    Err(CopyError::Read(err)) => Outcome::Failed(data_error(&err)),
                    Err(CopyError::Write(err)) => Outcome::Failed(ItemError::Io(err.to_string())),
                },
                Err(err) => Outcome::Failed(format_error(err)),
            }
        };
        report.record(entry, &outcome, handler);
    }
    Ok(report)
}

struct ExtractJob<'a> {
    archive: &'a mut Archive,
    dest: &'a Path,
    options: &'a ExtractOptions,
    overwrite: OverwriteMode,
    handler: &'a mut dyn ExtractHandler,
    ctl: &'a JobControl,
    report: Report,
    /// Destination-relative paths of every symbolic link in the archive.
    link_paths: HashSet<PathBuf>,
}

/// Where an item should go once an existing file has been dealt with.
enum Target {
    Write(PathBuf),
    Skip,
}

impl ExtractJob<'_> {
    fn finish(&mut self, entry: &Entry, outcome: &Outcome) {
        self.report.record(entry, outcome, self.handler);
    }

    fn make_folder(&mut self, rel: &Path) -> Outcome {
        if let Err(outcome) = self.prepare_parents(rel) {
            return outcome;
        }
        match fs::create_dir_all(self.dest.join(rel)) {
            Ok(()) => Outcome::Ok,
            Err(err) => Outcome::Failed(ItemError::Io(err.to_string())),
        }
    }

    fn write_file(&mut self, index: usize, entry: &Entry, rel: &Path) -> Result<Outcome> {
        if let Err(outcome) = self.prepare_parents(rel) {
            return Ok(outcome);
        }
        let path = match self.resolve_existing(&self.dest.join(rel), entry)? {
            Ok(Target::Write(path)) => path,
            Ok(Target::Skip) => return Ok(Outcome::Skipped),
            Err(err) => return Ok(Outcome::Failed(err)),
        };
        let stream = match self.archive.reader_mut().open_entry(index) {
            Ok(stream) => stream,
            Err(err) => return Ok(Outcome::Failed(format_error(err))),
        };
        let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => file,
            Err(err) => return Ok(Outcome::Failed(ItemError::Io(err.to_string()))),
        };
        let copied = copy_cancellable(stream, &mut file, self.ctl);
        let failure = match copied {
            Ok(bytes) => {
                self.report.bytes += bytes;
                apply_metadata(&file, &path, entry);
                return Ok(Outcome::Ok);
            }
            Err(CopyError::Cancelled) => None,
            Err(CopyError::Read(err)) => Some(data_error(&err)),
            Err(CopyError::Write(err)) => Some(ItemError::Io(err.to_string())),
        };
        drop(file);
        if failure.is_none() || !self.options.keep_broken {
            let _ = fs::remove_file(&path);
        }
        failure.map(Outcome::Failed).ok_or(Error::Cancelled)
    }

    fn make_link(&mut self, index: usize, rel: &Path) -> Result<Outcome> {
        let mut target = Vec::new();
        let read = match self.archive.reader_mut().open_entry(index) {
            Ok(stream) => stream.take(MAX_LINK_TARGET + 1).read_to_end(&mut target),
            Err(err) => return Ok(Outcome::Failed(format_error(err))),
        };
        if let Err(err) = read {
            return Ok(Outcome::Failed(data_error(&err)));
        }
        if target.len() as u64 > MAX_LINK_TARGET {
            return Ok(Outcome::Failed(ItemError::Data(
                "link target is too long".to_owned(),
            )));
        }
        let Ok(target) = String::from_utf8(target) else {
            return Ok(Outcome::Failed(ItemError::Unsupported(
                "non-UTF-8 link target".to_owned(),
            )));
        };
        let is_link = |path: &Path| {
            self.link_paths.contains(path)
                || fs::symlink_metadata(self.dest.join(path)).is_ok_and(|m| m.is_symlink())
        };
        if !link_stays_inside(rel, &target, is_link) {
            return Ok(Outcome::Failed(ItemError::UnsafeLink(target)));
        }
        if let Err(outcome) = self.prepare_parents(rel) {
            return Ok(outcome);
        }
        let entry = self.archive.entries()[index].clone();
        let path = match self.resolve_existing(&self.dest.join(rel), &entry)? {
            Ok(Target::Write(path)) => path,
            Ok(Target::Skip) => return Ok(Outcome::Skipped),
            Err(err) => return Ok(Outcome::Failed(err)),
        };
        Ok(match create_symlink(&target, &path) {
            Ok(()) => Outcome::Ok,
            Err(err) => Outcome::Failed(err),
        })
    }

    /// Creates the folders above `rel`, refusing to pass through a symbolic link.
    fn prepare_parents(&self, rel: &Path) -> std::result::Result<(), Outcome> {
        let mut current = self.dest.to_path_buf();
        let parents = rel.parent().into_iter().flat_map(Path::components);
        for component in parents {
            current.push(component);
            match fs::symlink_metadata(&current) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    return Err(Outcome::Failed(ItemError::UnsafePath));
                }
                Ok(meta) if meta.is_dir() => {}
                Ok(_) => {
                    return Err(Outcome::Failed(ItemError::Io(format!(
                        "{} exists and is not a folder",
                        current.display()
                    ))));
                }
                Err(_) => {
                    if let Err(err) = fs::create_dir(&current) {
                        return Err(Outcome::Failed(ItemError::Io(err.to_string())));
                    }
                }
            }
        }
        Ok(())
    }

    /// Applies the overwrite policy to `path`.
    fn resolve_existing(
        &mut self,
        path: &Path,
        entry: &Entry,
    ) -> Result<std::result::Result<Target, ItemError>> {
        let Ok(meta) = fs::symlink_metadata(path) else {
            return Ok(Ok(Target::Write(path.to_owned())));
        };
        if meta.is_dir() {
            return Ok(Err(ItemError::Io(format!(
                "{} is an existing folder",
                path.display()
            ))));
        }
        let mut mode = self.overwrite;
        if mode == OverwriteMode::Ask {
            mode = match self.handler.ask_overwrite(path, entry) {
                OverwriteChoice::Yes => OverwriteMode::Overwrite,
                OverwriteChoice::No => OverwriteMode::Skip,
                OverwriteChoice::Rename => OverwriteMode::RenameNew,
                OverwriteChoice::YesToAll => {
                    self.overwrite = OverwriteMode::Overwrite;
                    OverwriteMode::Overwrite
                }
                OverwriteChoice::NoToAll => {
                    self.overwrite = OverwriteMode::Skip;
                    OverwriteMode::Skip
                }
                OverwriteChoice::AutoRenameAll => {
                    self.overwrite = OverwriteMode::RenameNew;
                    OverwriteMode::RenameNew
                }
                OverwriteChoice::Cancel => return Err(Error::Cancelled),
            };
        }
        let io_err = |err: io::Error| ItemError::Io(err.to_string());
        Ok(match mode {
            OverwriteMode::Skip | OverwriteMode::Ask => Ok(Target::Skip),
            OverwriteMode::Overwrite => fs::remove_file(path)
                .map(|()| Target::Write(path.to_owned()))
                .map_err(io_err),
            OverwriteMode::RenameNew => Ok(Target::Write(unused_name(path))),
            OverwriteMode::RenameExisting => fs::rename(path, unused_name(path))
                .map(|()| Target::Write(path.to_owned()))
                .map_err(io_err),
        })
    }
}

enum CopyError {
    Cancelled,
    Read(io::Error),
    Write(io::Error),
}

fn copy_cancellable(
    mut from: impl Read,
    to: &mut impl Write,
    ctl: &JobControl,
) -> std::result::Result<u64, CopyError> {
    let mut buf = vec![0u8; COPY_BUFFER];
    let mut total = 0u64;
    loop {
        if ctl.is_cancelled() {
            return Err(CopyError::Cancelled);
        }
        let n = match from.read(&mut buf) {
            Ok(0) => return Ok(total),
            Ok(n) => n,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(CopyError::Read(err)),
        };
        to.write_all(&buf[..n]).map_err(CopyError::Write)?;
        total += n as u64;
    }
}

fn data_error(err: &io::Error) -> ItemError {
    match DataError::find(err) {
        Some(DataError::ChecksumMismatch) => ItemError::Checksum,
        Some(other) => ItemError::Data(other.to_string()),
        None => ItemError::Data(err.to_string()),
    }
}

fn format_error(err: FormatError) -> ItemError {
    match err {
        FormatError::Encrypted => ItemError::Encrypted,
        FormatError::Unsupported(what) => ItemError::Unsupported(what),
        other => ItemError::Data(other.to_string()),
    }
}

/// Whether `name` only consists of `.` components and separators.
fn is_current_dir(name: &str) -> bool {
    name.split(['/', '\\']).all(|part| matches!(part, "" | "."))
}

/// Picks `name_1.ext`, `name_2.ext`, … — the first that does not exist yet.
fn unused_name(path: &Path) -> PathBuf {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    (1..=u32::MAX)
        .map(|n| path.with_file_name(format!("{stem}_{n}{ext}")))
        .find(|candidate| fs::symlink_metadata(candidate).is_err())
        .unwrap_or_else(|| path.to_owned())
}

/// Whether a link at `rel` pointing to `target` resolves inside the destination.
///
/// The target is walked lexically from the link's folder. That is only sound if every
/// folder it passes through is a real folder, so passing through anything for which
/// `is_link` is true (another link in the archive, or an existing link on disk) is
/// rejected, as are absolute targets and `..` steps above the destination.
fn link_stays_inside(rel: &Path, target: &str, is_link: impl Fn(&Path) -> bool) -> bool {
    if target.is_empty() || target.starts_with(['/', '\\']) || Path::new(target).is_absolute() {
        return false;
    }
    if target.as_bytes().get(1) == Some(&b':') {
        return false;
    }
    let mut current: PathBuf = rel.parent().map(Path::to_path_buf).unwrap_or_default();
    let parts: Vec<&str> = target.split(['/', '\\']).collect();
    for (i, part) in parts.iter().enumerate() {
        match *part {
            "" | "." => {}
            ".." => {
                if !current.pop() {
                    return false;
                }
            }
            name => {
                current.push(name);
                let is_last = i + 1 == parts.len();
                if !is_last && is_link(&current) {
                    return false;
                }
            }
        }
    }
    true
}

#[cfg(unix)]
fn create_symlink(target: &str, path: &Path) -> std::result::Result<(), ItemError> {
    std::os::unix::fs::symlink(target, path).map_err(|err| ItemError::Io(err.to_string()))
}

/// Creates a native link where possible; without the privilege to create links
/// (common on Windows), stores a copy of the already-extracted target instead.
#[cfg(not(unix))]
fn create_symlink(target: &str, path: &Path) -> std::result::Result<(), ItemError> {
    let native = target.replace('/', "\\");
    let resolved = path.parent().unwrap_or(Path::new("")).join(&native);
    #[cfg(windows)]
    {
        let made = if resolved.is_dir() {
            std::os::windows::fs::symlink_dir(&native, path)
        } else {
            std::os::windows::fs::symlink_file(&native, path)
        };
        if made.is_ok() {
            return Ok(());
        }
    }
    if resolved.is_file() {
        return fs::copy(&resolved, path)
            .map(|_| ())
            .map_err(|err| ItemError::Io(err.to_string()));
    }
    Err(ItemError::Unsupported(
        "symbolic link: links cannot be created here and the target is not an extracted file"
            .to_owned(),
    ))
}

/// Restores modification time and permissions. Failures are not fatal: the data is
/// already safely on disk.
fn apply_metadata(file: &File, path: &Path, entry: &Entry) {
    if let Some(time) = entry.modified.and_then(to_system_time) {
        let _ = file.set_modified(time);
    }
    #[cfg(unix)]
    if let Some(mode) = entry.unix_mode {
        use std::os::unix::fs::PermissionsExt;
        // Never restore setuid/setgid/sticky bits from an archive.
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode & 0o777));
    }
    #[cfg(not(unix))]
    if entry.dos_attributes.is_some_and(|attrs| attrs & 1 != 0)
        && let Ok(mut perms) = file.metadata().map(|m| m.permissions())
    {
        perms.set_readonly(true);
        let _ = fs::set_permissions(path, perms);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_targets() {
        let no_links = |_: &Path| false;
        assert!(link_stays_inside(Path::new("a/link"), "target", no_links));
        assert!(link_stays_inside(Path::new("a/link"), "../b/c", no_links));
        assert!(!link_stays_inside(
            Path::new("a/link"),
            "../../etc",
            no_links
        ));
        assert!(!link_stays_inside(
            Path::new("link"),
            "/etc/passwd",
            no_links
        ));
        assert!(!link_stays_inside(
            Path::new("link"),
            "C:\\Windows",
            no_links
        ));
        assert!(!link_stays_inside(Path::new("link"), "..", no_links));
        assert!(link_stays_inside(Path::new("link"), "./x/../y", no_links));
    }

    #[test]
    fn link_through_another_link_is_rejected() {
        // `q -> .` is harmless alone, but `p -> q/..` would then resolve to the
        // destination's parent.
        let q_is_link = |p: &Path| p == Path::new("q");
        assert!(!link_stays_inside(Path::new("p"), "q/..", q_is_link));
        assert!(link_stays_inside(Path::new("p"), "q", q_is_link));
    }

    #[test]
    fn unused_name_appends_counter() {
        let name = unused_name(Path::new("/nonexistent/rarx/file.txt"));
        assert_eq!(name, Path::new("/nonexistent/rarx/file_1.txt"));
    }
}
