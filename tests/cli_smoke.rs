use std::process::Command;

fn bin_path() -> &'static str {
    env!("CARGO_BIN_EXE_grok-build")
}

#[test]
fn help_flag_succeeds_and_shows_usage() {
    let output = Command::new(bin_path())
        .arg("--help")
        .output()
        .expect("run --help");

    assert!(
        output.status.success(),
        "--help failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("grok-build"));
    assert!(stdout.contains("A Rust-based Grok coding CLI"));
}

#[test]
fn version_flag_succeeds_and_includes_binary_name() {
    let output = Command::new(bin_path())
        .arg("--version")
        .output()
        .expect("run --version");

    assert!(
        output.status.success(),
        "--version failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("grok-build"));
}
