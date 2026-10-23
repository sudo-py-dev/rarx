//! `rarx`: the WinRAR console `rar` dialect (AGENT.md §13.1–§13.2).

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use rarx_core::select::Selection;
use rarx_core::{
    Archive, Entry, EntryKind, Error, ExtractHandler, ExtractOptions, ItemError, JobControl,
    Outcome, OverwriteChoice, OverwriteMode, PathMode, Report, extract, test,
};

use crate::common::{
    Args, ask, display_name, dos_attribute_string, file_size, has_error, time_or_blank,
    unix_mode_string,
};
use crate::exit::Rar as Exit;

const USAGE: &str = "\
Usage:     rarx <command> -<switch 1> -<switch N> <archive> <files...>
               <@listfiles...> <path_to_extract/>

<Commands>
  e             Extract files without archived paths
  l[t[a],b]     List archive contents [technical[all], bare]
  p             Print file to stdout
  t             Test archive files
  v[t[a],b]     Verbosely list archive contents [technical[all], bare]
  x             Extract files with full path

<Switches>
  -             Stop switches scanning
  ep            Exclude paths from names
  idq           Disable messages, except errors
  inul          Disable all messages
  kb            Keep broken extracted files
  n<file>       Additionally filter included files
  o[+|-]        Set the overwrite mode
  op<path>      Set the output path for extracted files
  or            Rename files automatically
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
    let result = match command {
        "l" | "lt" | "lta" | "lb" | "v" | "vt" | "vta" | "vb" => {
            open(path).map(|mut a| list(&mut a, command, &options))
        }
        "t" => open(path).map(|mut a| run_test(&mut a, &options)),
        "x" if !options.exclude_paths => {
            open(path).map(|mut a| run_extract(&mut a, &options, PathMode::Full))
        }
        "x" | "e" => open(path).map(|mut a| run_extract(&mut a, &options, PathMode::None)),
        "p" => open(path).map(|mut a| print_files(&mut a, &options)),
        "a" | "c" | "ch" | "cw" | "d" | "f" | "i" | "k" | "m" | "mf" | "r" | "rc" | "rn" | "rr"
        | "rv" | "s" | "u" => {
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
    };
    for switch in &args.switches {
        let lower = switch.to_lowercase();
        match lower.as_str() {
            "y" => options.assume_yes = true,
            "o+" => options.overwrite = OverwriteMode::Overwrite,
            "o-" => options.overwrite = OverwriteMode::Skip,
            "or" => options.overwrite = OverwriteMode::RenameNew,
            "kb" => options.keep_broken = true,
            "ep" => options.exclude_paths = true,
            "idq" => options.quiet = true,
            "inul" => options.silent = true,
            "r" | "r-" | "r0" | "c-" | "cfg-" => {}
            _ if lower.starts_with("op") => options.output = Some(PathBuf::from(&switch[2..])),
            _ if lower.starts_with('x') && lower.len() > 1 => {
                options.selection = options.selection.exclude(&switch[1..]);
            }
            _ if lower.starts_with('n') && lower.len() > 1 => {
                options.selection = options.selection.include(&switch[1..]);
            }
            _ if lower.starts_with('p') || lower.starts_with("hp") => {
                return Err("Passwords are not supported yet".to_owned());
            }
            _ => return Err(format!("ERROR: Unknown option: {switch}")),
        }
    }
    let mut positionals = args.positionals.iter();
    let archive = positionals
        .next()
        .cloned()
        .ok_or("ERROR: Archive name is missing")?;
    for arg in positionals {
        if arg.ends_with(['/', '\\']) {
            options.output = Some(PathBuf::from(arg));
        } else {
            options.selection = options.selection.include(arg);
        }
    }
    Ok((options, archive))
}

fn open(path: &Path) -> Result<Archive, ExitCode> {
    if !path.exists() {
        eprintln!("Cannot open {}\nNo such file or directory", path.display());
        return Err(Exit::Open.into());
    }
    Archive::open(path).map_err(|err| {
        match err {
            Error::UnknownFormat(_) => eprintln!("{} is not an archive", path.display()),
            other => eprintln!("{}: {other}", path.display()),
        }
        Exit::Fatal.into()
    })
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
    println!(
        "Details: {}\n",
        format!("{:?}", archive.format()).to_uppercase()
    );
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
        ItemError::Encrypted => "encrypted file, passwords are not supported yet".to_owned(),
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
    } else if has_error(&report, |e| *e == ItemError::Encrypted) {
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
