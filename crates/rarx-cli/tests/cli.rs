use std::process::Command;

#[allow(clippy::unwrap_used)] // test helper; a failure here should abort the test
fn run(bin: &str, args: &[&str]) -> (Option<i32>, String) {
    let out = Command::new(bin).args(args).output().unwrap();
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

#[test]
fn version_flags() {
    let (code, stdout) = run(env!("CARGO_BIN_EXE_rarx"), &["--version"]);
    assert_eq!(code, Some(0));
    assert!(stdout.starts_with("rarx "));
    let (code, stdout) = run(env!("CARGO_BIN_EXE_rx7"), &["--version"]);
    assert_eq!(code, Some(0));
    assert!(stdout.starts_with("rx7 "));
}

#[test]
fn unimplemented_command_uses_dialect_exit_code() {
    assert_eq!(run(env!("CARGO_BIN_EXE_rarx"), &["a", "x.rar"]).0, Some(7));
    assert_eq!(run(env!("CARGO_BIN_EXE_rx7"), &["a", "x.7z"]).0, Some(7));
}
