//! `rarx`: the WinRAR console `rar` dialect (AGENT.md §13.1–§13.2).

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use rarx_core::select::Selection;
use rarx_core::{
    AddOptions, Archive, Entry, EntryKind, Error, ExtractHandler, ExtractOptions, FormatError,
    ItemAction, ItemError, JobControl, Outcome, OverwriteChoice, OverwriteMode, PathMode, Report,
    StorePaths, UpdateHandler, UpdateMode, ZipCipher, add, delete, extract, test,
};

use crate::common::{
    Args, PasswordArg, ask, display_name, dos_attribute_string, ensure_item_password, file_size,
    has_error, open_archive, time_or_blank, unix_mode_string,
};
use crate::exit::Rar as Exit;

const USAGE: &str = "\
Usage:     rarx <command> -<switch 1> -<switch N> <archive> <files...>
               <@listfiles...> <path_to_extract/>

<Commands>
  a             Add files to archive (only .zip archives can be created yet)
  d             Delete files from archive
  e             Extract files without archived paths
  f             Freshen files in archive
  l[t[a],b]     List archive contents [technical[all], bare]
  m[f]          Move to archive [files only]
  p             Print file to stdout
  t             Test archive files
  u             Update files in archive
  v[t[a],b]     Verbosely list archive contents [technical[all], bare]
  x             Extract files with full path

<Switches>
  -             Stop switches scanning
  df            Delete files after archiving
  ep            Exclude paths from names
  ep1           Exclude base directory from names
  ep2           Expand paths to full
  hp[password]  Encrypt both file data and headers
  idq           Disable messages, except errors
  inul          Disable all messages
  kb            Keep broken extracted files
  m<0..5>       Set compression level (0-store...3-default...5-maximal)
  n<file>       Additionally filter included files
  o[+|-]        Set the overwrite mode
  ol            Save symbolic links as the link instead of the file
  op<path>      Set the output path for extracted files
  or            Rename files automatically
  p[password]   Set password
  p-            Do not query password
  r             Recurse subdirectories (accepted; masks always match at any depth)
  x<file>       Exclude specified file
  y             Assume Yes on all queries";

const TIME_FORMAT: &str = "%Y-%m-%d %H:%M";

#[allow(clippy::struct_excessive_bools)] // one flag per command-line switch
struct Options {
    output: Option<PathBuf>,
    overwrite: OverwriteMode,
    assume_yes: bool,
    keep_broken: bool,
    exclude_paths: bool,
    quiet: bool,
    silent: bool,
    selection: Selection,
    excludes: Selection,
    level: u32,
    paths: StorePaths,
    store_symlinks: bool,
    delete_sources: bool,
    password: PasswordArg,
    encrypt_header: bool,
    /// Arguments after the archive name: files to add, or masks to extract.
    files: Vec<String>,
}

/// Runs the `rarx` command line.
#[must_use]
pub fn run(args: &[String]) -> ExitCode {
    let args = Args::parse(args);
    if args.command.is_empty() || matches!(args.command.as_str(), "-h" | "--help" | "-?" | "/?") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    if matches!(args.command.as_str(), "--version" | "-iver") {
        println!("rarx {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    let (options, archive_name) = match parse(&args) {
        Ok(parsed) => parsed,
        Err(message) => {
            eprintln!("{message}");
            return Exit::UserError.into();
        }
    };
    let path = Path::new(&archive_name);
    let bare = args.command.ends_with('b');
    if !options.silent && !options.quiet && !bare && args.command != "p" {
        println!("\nRARX {}   (RarX)\n", env!("CARGO_PKG_VERSION"));
    }

    let command = args.command.as_str();
    let result =
        match command {
            "l" | "lt" | "lta" | "lb" | "v" | "vt" | "vta" | "vb" => {
                open(path, &options, false).map(|mut a| list(&mut a, command, &options))
            }
            "t" => open(path, &options, true).map(|mut a| run_test(&mut a, &options)),
            "x" if !options.exclude_paths => open(path, &options, true)
                .map(|mut a| run_extract(&mut a, &options, PathMode::Full)),
            "x" | "e" => open(path, &options, true)
                .map(|mut a| run_extract(&mut a, &options, PathMode::None)),
            "p" => open(path, &options, true).map(|mut a| print_files(&mut a, &options)),
            "a" => return run_add(path, &options, UpdateMode::Add, false),
            "u" => return run_add(path, &options, UpdateMode::Update, false),
            "f" => return run_add(path, &options, UpdateMode::Freshen, false),
            "m" | "mf" => return run_add(path, &options, UpdateMode::Add, true),
            "d" => return run_delete(path, &options),
            "c" | "ch" | "cw" | "i" | "k" | "r" | "rc" | "rn" | "rr" | "rv" | "s" => {
                eprintln!("Command `{command}` is not implemented yet");
                return Exit::UserError.into();
            }
            other => {
                eprintln!("ERROR: Unknown command: {other}");
                return Exit::UserError.into();
            }
        };
    result.unwrap_or_else(|code| code)
}

fn parse(args: &Args) -> Result<(Options, String), String> {
    let mut options = Options {
        output: None,
        overwrite: OverwriteMode::Ask,
        assume_yes: false,
        keep_broken: false,
        exclude_paths: false,
        quiet: false,
        silent: false,
        selection: Selection::all(),
        excludes: Selection::all(),
        level: 6,
        paths: StorePaths::AsGiven,
        store_symlinks: false,
        delete_sources: false,
        password: PasswordArg::Unset,
        encrypt_header: false,
        files: Vec::new(),
    };
    for switch in &args.switches {
        let lower = switch.to_lowercase();
        match lower.as_str() {
            "y" => options.assume_yes = true,
            "o+" => options.overwrite = OverwriteMode::Overwrite,
            "o-" => options.overwrite = OverwriteMode::Skip,
            "or" => options.overwrite = OverwriteMode::RenameNew,
            "kb" => options.keep_broken = true,
            "ep" => {
                options.exclude_paths = true;
                options.paths = StorePaths::None;
            }
            "ep1" => options.paths = StorePaths::Relative,
            "ep2" | "ep3" => options.paths = StorePaths::AsGiven,
            "df" => options.delete_sources = true,
            "ol" => options.store_symlinks = true,
            "m0" | "m1" | "m2" | "m3" | "m4" | "m5" => {
                options.level = match lower.as_bytes()[1] {
                    b'0' => 0,
                    b'1' => 1,
                    b'2' => 3,
                    b'3' => 6,
                    b'4' => 8,
                    _ => 9,
                };
            }
            "idq" => options.quiet = true,
            "inul" => options.silent = true,
            "r" | "r-" | "r0" | "c-" | "cfg-" => {}
            _ if lower.starts_with("op") => options.output = Some(PathBuf::from(&switch[2..])),
            _ if lower.starts_with('x') && lower.len() > 1 => {
                options.selection = options.selection.exclude(&switch[1..]);
                options.excludes = options.excludes.exclude(&switch[1..]);
            }
            _ if lower.starts_with('n') && lower.len() > 1 => {
                options.selection = options.selection.include(&switch[1..]);
            }
            _ if lower.starts_with("hp") => {
                options.password = PasswordArg::from_switch(&switch[2..]);
                options.encrypt_header = true;
            }
            _ if lower.starts_with('p') => {
                options.password = PasswordArg::from_switch(&switch[1..]);
            }
            _ => return Err(format!("ERROR: Unknown option: {switch}")),
        }
    }
    let mut positionals = args.positionals.iter();
    let archive = positionals
        .next()
        .cloned()
        .ok_or("ERROR: Archive name is missing")?;
    let adding = matches!(args.command.as_str(), "a" | "u" | "f" | "m" | "mf");
    for arg in positionals {
        options.files.push(arg.clone());
        if adding {
            continue;
        }
        if arg.ends_with(['/', '\\']) {
            options.output = Some(PathBuf::from(arg));
        } else {
            options.selection = options.selection.include(arg);
        }
    }
    Ok((options, archive))
}

/// Prints one line per archive item the WinRAR way.
struct UpdateConsole {
    quiet: bool,
}

impl UpdateHandler for UpdateConsole {
    fn item_done(&mut self, name: &str, action: ItemAction) {
        let verb = match action {
            ItemAction::Added => "Adding",
            ItemAction::Replaced => "Updating",
            ItemAction::Deleted => "Deleting",
            ItemAction::Kept => return,
        };
        if !self.quiet {
            println!("{verb:<11} {:<56} OK", display_name(name));
        }
    }

    fn warning(&mut self, path: &Path, message: &str) {
        eprintln!("WARNING: Cannot open {}\n{message}", path.display());
    }
}

/// RAR appends `.rar` to names without an extension; RarX cannot write RAR yet.
fn creatable(path: &Path) -> Result<(), ExitCode> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default();
    if ext.is_empty() || ext.eq_ignore_ascii_case("rar") {
        eprintln!("ERROR: creating RAR archives is not available yet; use a .zip archive name");
        return Err(Exit::Create.into());
    }
    Ok(())
}

fn run_add(path: &Path, options: &Options, mode: UpdateMode, move_files: bool) -> ExitCode {
    if let Err(code) = creatable(path) {
        return code;
    }
    if options.files.is_empty() {
        eprintln!("ERROR: No files to add");
        return Exit::UserError.into();
    }
    let quiet = options.quiet || options.silent;
    let password = match options.password.initial(true) {
        Ok(password) => password,
        Err(message) => {
            eprintln!("ERROR: {message}");
            return Exit::UserError.into();
        }
    };
    if !options.silent {
        let verb = if path.exists() {
            "Updating"
        } else {
            "Creating"
        };
        println!("{verb} archive {}\n", path.display());
    }
    let inputs: Vec<PathBuf> = options.files.iter().map(PathBuf::from).collect();
    let add_options = AddOptions {
        level: options.level,
        paths: options.paths,
        mode,
        exclude: options.excludes.clone(),
        store_symlinks: options.store_symlinks,
        delete_sources: options.delete_sources || move_files,
        solid: true,
        compress_header: true,
        force_zip64: false,
        password,
        encrypt_header: options.encrypt_header,
        zip_cipher: ZipCipher::Aes256,
    };
    let mut console = UpdateConsole { quiet };
    match add(
        path,
        &inputs,
        &add_options,
        &mut console,
        &JobControl::default(),
    ) {
        Ok(report) if !report.written => {
            if !options.silent {
                println!("WARNING: No files");
            }
            Exit::NoFiles.into()
        }
        Ok(report) => {
            if !options.silent {
                println!("Done");
            }
            if report.warnings.is_empty() {
                ExitCode::SUCCESS
            } else {
                Exit::Warning.into()
            }
        }
        Err(Error::Cancelled) => Exit::UserBreak.into(),
        Err(err) => {
            eprintln!("ERROR: {err}");
            Exit::Fatal.into()
        }
    }
}

fn run_delete(path: &Path, options: &Options) -> ExitCode {
    if options.files.is_empty() {
        eprintln!("ERROR: No files to delete");
        return Exit::UserError.into();
    }
    if !path.exists() {
        eprintln!("Cannot open {}\nNo such file or directory", path.display());
        return Exit::Open.into();
    }
    if !options.silent {
        println!("Deleting from {}\n", path.display());
    }
    let password = match options.password.initial(false) {
        Ok(password) => password,
        Err(message) => {
            eprintln!("ERROR: {message}");
            return Exit::UserError.into();
        }
    };
    let mut console = UpdateConsole {
        quiet: options.quiet || options.silent,
    };
    match delete(
        path,
        &options.selection,
        password,
        &mut console,
        &JobControl::default(),
    ) {
        Ok(report) if report.deleted == 0 => {
            if !options.silent {
                println!("No files to delete");
            }
            Exit::NoFiles.into()
        }
        Ok(_) => {
            if !options.silent {
                println!("Done");
            }
            ExitCode::SUCCESS
        }
        Err(Error::Cancelled) => Exit::UserBreak.into(),
        Err(err) => {
            eprintln!("ERROR: {err}");
            Exit::Fatal.into()
        }
    }
}

/// Opens the archive, asking for a password when the header (or, with
/// `reads_data`, any item) is encrypted.
fn open(path: &Path, options: &Options, reads_data: bool) -> Result<Archive, ExitCode> {
    if !path.exists() {
        eprintln!("Cannot open {}\nNo such file or directory", path.display());
        return Err(Exit::Open.into());
    }
    let mut password = options.password.initial(false).map_err(|message| {
        eprintln!("ERROR: {message}");
        ExitCode::from(Exit::UserError)
    })?;
    let mut archive =
        open_archive(path, &options.password, &mut password).map_err(|err| -> ExitCode {
            match err {
                Error::UnknownFormat(_) => {
                    eprintln!("{} is not an archive", path.display());
                    Exit::Fatal.into()
                }
                Error::Format(FormatError::Encrypted | FormatError::WrongPassword) => {
                    eprintln!("The specified password is incorrect.");
                    Exit::BadPassword.into()
                }
                other => {
                    eprintln!("{}: {other}", path.display());
                    Exit::Fatal.into()
                }
            }
        })?;
    if reads_data {
        ensure_item_password(&mut archive, &options.password, &mut password);
    }
    Ok(archive)
}

fn list(archive: &mut Archive, command: &str, options: &Options) -> ExitCode {
    let selected: Vec<&Entry> = archive
        .entries()
        .iter()
        .filter(|e| options.selection.matches(&e.name))
        .collect();
    if command.ends_with('b') {
        for entry in selected {
            println!("{}", display_name(&entry.name));
        }
        return ExitCode::SUCCESS;
    }
    println!("Archive: {}", archive.path().display());
    println!("Details: {}\n", archive.format().name().to_uppercase());
    if let Some(comment) = archive.comment() {
        println!("{comment}\n");
    }
    if command.contains('t') {
        for entry in selected {
            print_technical(entry);
        }
        return ExitCode::SUCCESS;
    }

    let rule = "----------- ---------  ---------- -----  ----";
    println!(" Attributes      Size     Date    Time   Name");
    println!("{rule}");
    let (mut total, mut count) = (0u64, 0u64);
    for entry in selected {
        println!(
            " {:<10} {:>9}  {:<16}  {}",
            attributes(entry),
            entry.size,
            time_or_blank(entry.modified, TIME_FORMAT, 16),
            display_name(&entry.name)
        );
        total += entry.size;
        count += 1;
    }
    println!("{rule}");
    println!("            {total:>9}                    {count}");
    ExitCode::SUCCESS
}

fn attributes(entry: &Entry) -> String {
    unix_mode_string(entry).unwrap_or_else(|| dos_attribute_string(entry))
}

fn print_technical(entry: &Entry) {
    let kind = match entry.kind {
        EntryKind::File => "File",
        EntryKind::Directory => "Directory",
        EntryKind::Symlink => "Symbolic link",
    };
    println!("        Name: {}", display_name(&entry.name));
    println!("        Type: {kind}");
    println!("        Size: {}", entry.size);
    println!(" Packed size: {}", entry.packed_size);
    if let Some(ratio) = entry
        .packed_size
        .saturating_mul(100)
        .checked_div(entry.size)
    {
        println!("       Ratio: {ratio}%");
    }
    println!(
        "       mtime: {}",
        time_or_blank(entry.modified, "%Y-%m-%d %H:%M:%S", 0)
    );
    println!("  Attributes: {}", attributes(entry));
    if let Some(crc) = entry.crc32 {
        println!("       CRC32: {crc:08X}");
    }
    println!(" Compression: {}", entry.method);
    if entry.encrypted {
        println!("       Flags: encrypted");
    }
    println!();
}

/// Prints per-file progress and answers overwrite questions the WinRAR way.
struct Console {
    verb: &'static str,
    assume_yes: bool,
    quiet: bool,
}

impl ExtractHandler for Console {
    fn item_done(&mut self, entry: &Entry, outcome: &Outcome) {
        let name = display_name(&entry.name);
        match outcome {
            Outcome::Ok if !self.quiet => {
                let verb = if entry.is_dir() {
                    "Creating"
                } else {
                    self.verb
                };
                println!("{verb:<11} {name:<56} OK");
            }
            Outcome::Ok | Outcome::Skipped => {}
            Outcome::Failed(err) => eprintln!("{name:<68} - {}", describe(err)),
        }
    }

    fn ask_overwrite(&mut self, path: &Path, entry: &Entry) -> OverwriteChoice {
        if self.assume_yes {
            return OverwriteChoice::Yes;
        }
        let question = format!(
            "\nWould you like to replace the existing file {}\n{:>10} bytes\n\n\
             with a new one\n{:>10} bytes, modified on {}\n\n\
             [Y]es, [N]o, [A]ll, n[E]ver, [R]ename, [Q]uit ",
            path.display(),
            file_size(path),
            entry.size,
            time_or_blank(entry.modified, TIME_FORMAT, 0),
        );
        loop {
            return match ask(&question).as_deref() {
                Some("y") => OverwriteChoice::Yes,
                Some("n") => OverwriteChoice::No,
                Some("a") => OverwriteChoice::YesToAll,
                Some("e") | None => OverwriteChoice::NoToAll,
                Some("r") => OverwriteChoice::Rename,
                Some("q") => OverwriteChoice::Cancel,
                Some(_) => continue,
            };
        }
    }
}

fn describe(err: &ItemError) -> String {
    match err {
        ItemError::Checksum => "checksum error".to_owned(),
        ItemError::Encrypted => "encrypted file, a password is needed (-p)".to_owned(),
        ItemError::WrongPassword => "incorrect password".to_owned(),
        ItemError::UnsafePath => "unsafe path, skipped".to_owned(),
        ItemError::UnsafeLink(target) => format!("dangerous link to \"{target}\" skipped"),
        other => other.to_string(),
    }
}

fn run_test(archive: &mut Archive, options: &Options) -> ExitCode {
    if !options.silent {
        println!("Testing archive {}\n", archive.path().display());
    }
    let mut console = Console {
        verb: "Testing",
        assume_yes: options.assume_yes,
        quiet: options.quiet || options.silent,
    };
    let result = test(
        archive,
        &options.selection,
        &mut console,
        &JobControl::default(),
    );
    finish(result, options)
}

fn run_extract(archive: &mut Archive, options: &Options, paths: PathMode) -> ExitCode {
    if !options.silent {
        println!("Extracting from {}\n", archive.path().display());
    }
    let dest = options.output.clone().unwrap_or_else(|| PathBuf::from("."));
    let extract_options = ExtractOptions {
        paths,
        overwrite: options.overwrite,
        selection: options.selection.clone(),
        keep_broken: options.keep_broken,
        ..ExtractOptions::default()
    };
    let mut console = Console {
        verb: "Extracting",
        assume_yes: options.assume_yes,
        quiet: options.quiet || options.silent,
    };
    let result = extract(
        archive,
        &dest,
        &extract_options,
        &mut console,
        &JobControl::default(),
    );
    finish(result, options)
}

fn finish(result: rarx_core::Result<Report>, options: &Options) -> ExitCode {
    let report = match result {
        Ok(report) => report,
        Err(Error::Cancelled) => {
            eprintln!("User break");
            return Exit::UserBreak.into();
        }
        Err(err) => {
            eprintln!("{err}");
            return Exit::Fatal.into();
        }
    };
    let processed = report.files + report.folders + report.links + report.skipped;
    if processed == 0 && report.errors.is_empty() {
        if !options.silent {
            println!("No files to extract");
        }
        return Exit::NoFiles.into();
    }
    if report.errors.is_empty() {
        if !options.silent {
            println!("All OK");
        }
        return ExitCode::SUCCESS;
    }
    eprintln!("Total errors: {}", report.errors.len());
    if has_error(&report, |e| *e == ItemError::Checksum) {
        Exit::Crc.into()
    } else if has_error(&report, |e| {
        matches!(e, ItemError::Encrypted | ItemError::WrongPassword)
    }) {
        Exit::BadPassword.into()
    } else {
        Exit::Fatal.into()
    }
}

fn print_files(archive: &mut Archive, options: &Options) -> ExitCode {
    let indices: Vec<usize> = archive
        .entries()
        .iter()
        .enumerate()
        .filter(|(_, e)| e.kind == EntryKind::File && options.selection.matches(&e.name))
        .map(|(i, _)| i)
        .collect();
    if indices.is_empty() {
        return Exit::NoFiles.into();
    }
    let mut stdout = io::stdout().lock();
    let mut code = ExitCode::SUCCESS;
    for index in indices {
        let name = display_name(&archive.entries()[index].name);
        let copied = archive
            .read_entry(index)
            .map_err(|e| e.to_string())
            .and_then(|mut stream| io::copy(&mut stream, &mut stdout).map_err(|e| e.to_string()));
        if let Err(message) = copied {
            eprintln!("{name} - {message}");
            code = Exit::Fatal.into();
        }
    }
    let _ = stdout.flush();
    code
}
