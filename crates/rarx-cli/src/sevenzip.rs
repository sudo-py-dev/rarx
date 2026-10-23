//! `rx7`: the 7-Zip `7z` command-line dialect (AGENT.md §13.3–§13.4).

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use rarx_core::select::Selection;
use rarx_core::time::to_system_time;
use rarx_core::{
    AddOptions, Archive, Entry, Error, ExtractHandler, ExtractOptions, ItemError, JobControl,
    Outcome, OverwriteChoice, OverwriteMode, PathMode, Report, StorePaths, UpdateHandler,
    UpdateMode, add, delete, extract, test,
};

use crate::common::{
    Args, ask, display_name, dos_attribute_string, file_size, time_or_blank, unix_mode_string,
};
use crate::exit::SevenZip as Exit;

const USAGE: &str = "\
Usage: rx7 <command> [<switches>...] <archive_name> [<file_names>...]

<Commands>
  a : Add files to archive
  d : Delete files from archive
  e : Extract files from archive (without using directory names)
  l : List contents of archive
  t : Test integrity of archive
  u : Update files to archive
  x : eXtract files with full paths

<Switches>
  -- : Stop switches parsing
  -ao{a|s|t|u} : set Overwrite mode
  -i!{wildcard} : Include filenames
  -m{Parameters} : set compression Method (-mx[0-9], -mm=Copy|Deflate)
  -o{Directory} : set Output directory
  -r[-] : Recurse subdirectories (accepted; masks always match at any depth)
  -sdel : delete files after compression
  -slt : show technical information for l (List) command
  -snl : store symbolic links as links
  -so : write data to stdout
  -t{Type} : Set type of archive (only zip can be created yet)
  -x!{wildcard} : eXclude filenames
  -y : assume Yes on all queries";

const TIME_FORMAT: &str = "%Y-%m-%d %H:%M:%S";

#[allow(clippy::struct_excessive_bools)] // one flag per command-line switch
struct Options {
    output: Option<PathBuf>,
    overwrite: OverwriteMode,
    assume_yes: bool,
    technical: bool,
    to_stdout: bool,
    selection: Selection,
    excludes: Selection,
    level: u32,
    store_symlinks: bool,
    delete_sources: bool,
    archive_type: Option<String>,
}

/// Runs the `rx7` command line.
#[must_use]
pub fn run(args: &[String]) -> ExitCode {
    let args = Args::parse(args);
    if args.command.is_empty() || matches!(args.command.as_str(), "-h" | "--help" | "-?") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    if matches!(args.command.as_str(), "--version" | "-version") {
        println!("rx7 {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    let options = match parse_switches(&args) {
        Ok(options) => options,
        Err(message) => return command_line_error(&message),
    };
    let Some(archive_name) = args.positionals.first() else {
        return command_line_error("Cannot find archive name");
    };
    let path = Path::new(archive_name);

    if !options.to_stdout {
        println!("\nrx7 {} (RarX)\n", env!("CARGO_PKG_VERSION"));
    }
    match args.command.as_str() {
        "l" => with_archive(path, |a| list(a, &options)),
        "t" => with_archive(path, |a| run_test(a, &options)),
        "x" => with_archive(path, |a| run_extract(a, &options, PathMode::Full)),
        "e" => with_archive(path, |a| run_extract(a, &options, PathMode::None)),
        "a" => run_add(path, &args.positionals[1..], &options, UpdateMode::Add),
        "u" => run_add(path, &args.positionals[1..], &options, UpdateMode::Update),
        "d" => run_delete(path, &args.positionals[1..], &options),
        "rn" | "h" | "b" | "i" => command_line_error(&format!(
            "Command `{}` is not implemented yet",
            args.command
        )),
        other => command_line_error(&format!("Unsupported command: {other}")),
    }
}

fn parse_switches(args: &Args) -> Result<Options, String> {
    let mut options = Options {
        output: None,
        overwrite: OverwriteMode::Ask,
        assume_yes: false,
        technical: false,
        to_stdout: false,
        selection: Selection::all(),
        excludes: Selection::all(),
        level: 6,
        store_symlinks: false,
        delete_sources: false,
        archive_type: None,
    };
    for switch in &args.switches {
        let lower = switch.to_lowercase();
        match lower.as_str() {
            "y" => options.assume_yes = true,
            "slt" => options.technical = true,
            "so" => options.to_stdout = true,
            "r" | "r-" | "r0" | "bd" | "mm=deflate" | "m0=deflate" => {}
            "sdel" => options.delete_sources = true,
            "snl" => options.store_symlinks = true,
            "mm=copy" | "m0=copy" => options.level = 0,
            _ if lower.starts_with("mx") => {
                let digits = lower[2..].trim_start_matches('=');
                let mx: u32 = if digits.is_empty() {
                    9
                } else {
                    digits
                        .parse()
                        .map_err(|_| format!("Unsupported switch: -{switch}"))?
                };
                options.level = deflate_level(mx);
            }
            _ if lower.starts_with('m') => {
                return Err(format!("Unsupported compression parameter: -{switch}"));
            }
            _ if lower.starts_with('t') => options.archive_type = Some(lower[1..].to_owned()),
            "aoa" => options.overwrite = OverwriteMode::Overwrite,
            "aos" => options.overwrite = OverwriteMode::Skip,
            "aot" => options.overwrite = OverwriteMode::RenameExisting,
            "aou" => options.overwrite = OverwriteMode::RenameNew,
            _ if lower.starts_with("bb") => {}
            _ if lower.starts_with('o') => options.output = Some(PathBuf::from(&switch[1..])),
            _ if lower.starts_with("x!") => {
                options.selection = options.selection.exclude(&switch[2..]);
                options.excludes = options.excludes.exclude(&switch[2..]);
            }
            _ if lower.starts_with("i!") => {
                options.selection = options.selection.include(&switch[2..]);
            }
            _ if lower.starts_with('p') => {
                return Err("Passwords are not supported yet".to_owned());
            }
            _ => return Err(format!("Unsupported switch: -{switch}")),
        }
    }
    for mask in args.positionals.iter().skip(1) {
        options.selection = options.selection.include(mask);
    }
    Ok(options)
}

/// Maps 7-Zip `-mx` levels (0–9) onto Deflate levels.
fn deflate_level(mx: u32) -> u32 {
    match mx {
        0 => 0,
        1..=3 => mx,
        4 => 5,
        5 => 6,
        6 => 7,
        _ => 9,
    }
}

/// Prints added items and unreadable sources the 7-Zip way.
struct UpdateConsole;

impl UpdateHandler for UpdateConsole {
    fn warning(&mut self, path: &Path, message: &str) {
        eprintln!("WARNING: {message} : {}", path.display());
    }
}

fn run_add(path: &Path, inputs: &[String], options: &Options, mode: UpdateMode) -> ExitCode {
    if let Some(kind) = options.archive_type.as_deref().filter(|t| *t != "zip") {
        eprintln!("ERROR: creating {kind} archives is not supported yet");
        return Exit::Fatal.into();
    }
    let inputs: Vec<PathBuf> = if inputs.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        inputs.iter().map(PathBuf::from).collect()
    };
    let exists = path.exists();
    println!(
        "{} archive: {}
",
        if exists { "Updating" } else { "Creating" },
        path.display()
    );
    let add_options = AddOptions {
        level: options.level,
        paths: StorePaths::Relative,
        mode,
        exclude: options.excludes.clone(),
        store_symlinks: options.store_symlinks,
        delete_sources: options.delete_sources,
        force_zip64: false,
    };
    match add(
        path,
        &inputs,
        &add_options,
        &mut UpdateConsole,
        &JobControl::default(),
    ) {
        Ok(report) => {
            println!("Files read from disk: {}", report.added + report.replaced);
            println!("Archive size: {} bytes", file_size(path));
            if !report.written {
                println!("No files to process");
            }
            if report.warnings.is_empty() {
                println!("Everything is Ok");
                ExitCode::SUCCESS
            } else {
                println!("\nWARNINGS for files:\n");
                for (file, message) in &report.warnings {
                    println!("{} : {message}", file.display());
                }
                println!(
                    "----------------\nWARNING: Cannot find {} file(s)",
                    report.warnings.len()
                );
                Exit::Warning.into()
            }
        }
        Err(Error::Cancelled) => Exit::UserBreak.into(),
        Err(err) => {
            eprintln!("ERROR: {}\n{err}", path.display());
            Exit::Fatal.into()
        }
    }
}

fn run_delete(path: &Path, masks: &[String], options: &Options) -> ExitCode {
    if masks.is_empty() {
        return command_line_error("Cannot find files to delete; specify file names or masks");
    }
    let selection = masks.iter().fold(Selection::all(), |sel, m| sel.include(m));
    let selection = options.excludes_applied(selection);
    println!("Updating archive: {}\n", path.display());
    match delete(path, &selection, &mut UpdateConsole, &JobControl::default()) {
        Ok(report) => {
            println!("Deleted: {}", report.deleted);
            println!("Everything is Ok");
            ExitCode::SUCCESS
        }
        Err(Error::Cancelled) => Exit::UserBreak.into(),
        Err(err) => {
            eprintln!("ERROR: {}\n{err}", path.display());
            Exit::Fatal.into()
        }
    }
}

impl Options {
    /// Adds the `-x!` masks to an include-only selection.
    fn excludes_applied(&self, selection: Selection) -> Selection {
        self.excludes
            .exclusions()
            .iter()
            .fold(selection, |sel, m| sel.exclude(m))
    }
}

fn command_line_error(message: &str) -> ExitCode {
    eprintln!("\nCommand Line Error:\n{message}");
    Exit::CommandLineError.into()
}

/// Opens the archive and runs `body`, printing 7-Zip style open errors.
fn with_archive(path: &Path, body: impl FnOnce(&mut Archive) -> ExitCode) -> ExitCode {
    if !path.exists() {
        eprintln!(
            "ERROR: {}\nThe system cannot find the file specified.",
            path.display()
        );
        return Exit::Fatal.into();
    }
    match Archive::open(path) {
        Ok(mut archive) => body(&mut archive),
        Err(Error::UnknownFormat(_)) => {
            eprintln!("ERROR: {}\nCannot open the file as archive", path.display());
            Exit::Fatal.into()
        }
        Err(err) => {
            eprintln!("ERROR: {}\n{err}", path.display());
            Exit::Fatal.into()
        }
    }
}

fn print_archive_header(archive: &Archive) {
    println!("--");
    println!("Path = {}", archive.path().display());
    println!("Type = {}", format_name(archive));
    println!("Physical Size = {}", file_size(archive.path()));
    if let Some(comment) = archive.comment() {
        println!("Comment = {comment}");
    }
    println!();
}

fn format_name(archive: &Archive) -> String {
    archive.format().name().to_owned()
}

fn list(archive: &mut Archive, options: &Options) -> ExitCode {
    println!("Listing archive: {}\n", archive.path().display());
    print_archive_header(archive);
    let selected: Vec<&Entry> = archive
        .entries()
        .iter()
        .filter(|e| options.selection.matches(&e.name))
        .collect();
    if options.technical {
        println!("----------");
        for entry in selected {
            print_technical(entry);
        }
        return ExitCode::SUCCESS;
    }

    let rule = "------------------- ----- ------------ ------------  ------------------------";
    println!("   Date      Time    Attr         Size   Compressed  Name");
    println!("{rule}");
    let (mut size, mut packed, mut files, mut folders) = (0u64, 0u64, 0u64, 0u64);
    let mut newest = None;
    for entry in selected {
        let time = time_or_blank(entry.modified, TIME_FORMAT, 19);
        if let Some(when) = entry.modified.and_then(to_system_time)
            && newest.as_ref().is_none_or(|(latest, _)| when > *latest)
        {
            newest = Some((when, time.clone()));
        }
        println!(
            "{time} {} {:>12} {:>12}  {}",
            dos_attribute_string(entry),
            entry.size,
            entry.packed_size,
            display_name(&entry.name)
        );
        if entry.is_dir() {
            folders += 1;
        } else {
            files += 1;
            size += entry.size;
            packed += entry.packed_size;
        }
    }
    println!("{rule}");
    let newest = newest.map_or_else(|| " ".repeat(19), |(_, text)| text);
    println!("{newest}       {size:>12} {packed:>12}  {files} files, {folders} folders");
    ExitCode::SUCCESS
}

fn print_technical(entry: &Entry) {
    let yes_no = |flag: bool| if flag { "+" } else { "-" };
    println!();
    println!("Path = {}", display_name(&entry.name));
    println!("Folder = {}", yes_no(entry.is_dir()));
    println!("Size = {}", entry.size);
    println!("Packed Size = {}", entry.packed_size);
    println!(
        "Modified = {}",
        time_or_blank(entry.modified, TIME_FORMAT, 0)
    );
    if let Some(created) = entry.created {
        println!("Created = {}", time_or_blank(Some(created), TIME_FORMAT, 0));
    }
    if let Some(accessed) = entry.accessed {
        println!(
            "Accessed = {}",
            time_or_blank(Some(accessed), TIME_FORMAT, 0)
        );
    }
    let unix = unix_mode_string(entry)
        .map(|m| format!(" {m}"))
        .unwrap_or_default();
    println!("Attributes = {}{unix}", dos_attribute_string(entry));
    println!("Encrypted = {}", yes_no(entry.encrypted));
    println!("Comment = {}", entry.comment.as_deref().unwrap_or_default());
    println!(
        "CRC = {}",
        entry.crc32.map(|c| format!("{c:08X}")).unwrap_or_default()
    );
    println!("Method = {}", entry.method);
}

/// Prints progress lines and answers overwrite questions on the console.
struct Console {
    assume_yes: bool,
}

impl ExtractHandler for Console {
    fn item_done(&mut self, entry: &Entry, outcome: &Outcome) {
        if let Outcome::Failed(err) = outcome {
            eprintln!("ERROR: {} : {}", describe(err), display_name(&entry.name));
        }
    }

    fn ask_overwrite(&mut self, path: &Path, entry: &Entry) -> OverwriteChoice {
        if self.assume_yes {
            return OverwriteChoice::Yes;
        }
        let question = format!(
            "\nWould you like to replace the existing file:\n  Path:     {}\n  Size:     {} bytes\n\
             with the file from archive:\n  Path:     {}\n  Size:     {} bytes\n\
             ? (Y)es / (N)o / (A)lways / (S)kip all / A(u)to rename all / (Q)uit? ",
            path.display(),
            file_size(path),
            display_name(&entry.name),
            entry.size,
        );
        loop {
            return match ask(&question).as_deref() {
                Some("y") => OverwriteChoice::Yes,
                Some("n") => OverwriteChoice::No,
                Some("a") => OverwriteChoice::YesToAll,
                Some("s") | None => OverwriteChoice::NoToAll,
                Some("u") => OverwriteChoice::AutoRenameAll,
                Some("q") => OverwriteChoice::Cancel,
                Some(_) => continue,
            };
        }
    }
}

fn describe(err: &ItemError) -> String {
    match err {
        ItemError::Checksum => "CRC Failed".to_owned(),
        ItemError::Encrypted => "Wrong password or encrypted data".to_owned(),
        ItemError::UnsafePath => "Dangerous path".to_owned(),
        ItemError::UnsafeLink(target) => format!("Dangerous link path was ignored ({target})"),
        other => other.to_string(),
    }
}

fn run_test(archive: &mut Archive, options: &Options) -> ExitCode {
    println!("Testing archive: {}", archive.path().display());
    print_archive_header(archive);
    let mut console = Console {
        assume_yes: options.assume_yes,
    };
    let result = test(
        archive,
        &options.selection,
        &mut console,
        &JobControl::default(),
    );
    finish(archive, result)
}

fn run_extract(archive: &mut Archive, options: &Options, paths: PathMode) -> ExitCode {
    if options.to_stdout {
        return extract_to_stdout(archive, options);
    }
    println!("Extracting archive: {}", archive.path().display());
    print_archive_header(archive);
    let dest = options.output.clone().unwrap_or_else(|| PathBuf::from("."));
    let extract_options = ExtractOptions {
        paths,
        overwrite: options.overwrite,
        selection: options.selection.clone(),
        ..ExtractOptions::default()
    };
    let mut console = Console {
        assume_yes: options.assume_yes,
    };
    let ctl = JobControl::default();
    let result = extract(archive, &dest, &extract_options, &mut console, &ctl);
    finish(archive, result)
}

fn finish(archive: &Archive, result: rarx_core::Result<Report>) -> ExitCode {
    let report = match result {
        Ok(report) => report,
        Err(Error::Cancelled) => {
            eprintln!("\nBreak signaled");
            return Exit::UserBreak.into();
        }
        Err(err) => {
            eprintln!("ERROR: {err}");
            return Exit::Fatal.into();
        }
    };
    if report.files + report.folders + report.links + report.skipped == 0
        && report.errors.is_empty()
    {
        println!("No files to process");
    }
    if report.errors.is_empty() {
        println!("Everything is Ok\n");
    } else {
        println!("\nSub items Errors: {}\n", report.errors.len());
    }
    if report.folders > 0 {
        println!("Folders: {}", report.folders);
    }
    println!("Files: {}", report.files + report.links);
    println!("Size:       {}", report.bytes);
    println!("Compressed: {}", file_size(archive.path()));
    if report.errors.is_empty() {
        ExitCode::SUCCESS
    } else {
        Exit::Fatal.into()
    }
}

fn extract_to_stdout(archive: &mut Archive, options: &Options) -> ExitCode {
    let indices: Vec<usize> = archive
        .entries()
        .iter()
        .enumerate()
        .filter(|(_, e)| !e.is_dir() && options.selection.matches(&e.name))
        .map(|(i, _)| i)
        .collect();
    let mut stdout = io::stdout().lock();
    let mut failed = false;
    for index in indices {
        let name = display_name(&archive.entries()[index].name);
        let copied = archive
            .read_entry(index)
            .map_err(|e| e.to_string())
            .and_then(|mut stream| io::copy(&mut stream, &mut stdout).map_err(|e| e.to_string()));
        if let Err(message) = copied {
            eprintln!("ERROR: {message} : {name}");
            failed = true;
        }
    }
    let _ = stdout.flush();
    if failed {
        Exit::Fatal.into()
    } else {
        ExitCode::SUCCESS
    }
}
