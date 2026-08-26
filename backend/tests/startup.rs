use std::process::Command;

#[test]
fn binary_exits_successfully_without_output() {
    let output = Command::new(env!("CARGO_BIN_EXE_craxii-server"))
        .output()
        .expect("craxii-server binary should execute");

    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}
