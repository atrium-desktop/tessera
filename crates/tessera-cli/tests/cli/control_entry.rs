//! The headless executable uses the shared command model without a desktop runtime.
use std::process::Command;

fn command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tessera-ctl"));
    command.env_remove("XDG_RUNTIME_DIR");
    command
}

#[test]
fn local_commands_do_not_require_a_session() {
    let output = command().args(["calc", "25 * 4"]).output().unwrap();
    assert!(output.status.success(), "{:?}", output);
    assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), "100");
    let output = command()
        .args(["query", "100 / 2", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output);
    let rows: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(rows[0]["title"], "= 50");
}

#[test]
fn headless_entry_requires_a_command_and_cannot_start_the_compositor() {
    assert!(!command().output().unwrap().status.success());
    assert!(!command().arg("run").output().unwrap().status.success());
    let output = command().arg("--help").output().unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(help.contains("window"));
    assert!(help.contains("system"));
    assert!(!help.contains("exec "));
}
