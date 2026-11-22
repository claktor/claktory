//! The helper as a program: run by hand, or for another extension, it
//! refuses before touching anything.

use std::process::{Command, Stdio};

fn helper(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_claktory-browser-host"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("the helper runs")
}

#[test]
fn run_by_hand_or_for_another_extension_it_refuses_and_says_nothing_on_stdout() {
    for args in [&[][..], &["--serve"], &["http://example.com/"], &["chrome-extension://someoneelse/"]] {
        let out = helper(args);
        assert!(!out.status.success(), "{args:?}");
        assert!(out.stdout.is_empty(), "{args:?}: stdout is Chrome's framing only");
        assert!(String::from_utf8_lossy(&out.stderr).starts_with("claktory-browser-host: "), "{args:?}");
    }
    let out = helper(&["--register", "--hosts-dir", "/tmp"]);
    assert!(!out.status.success());
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("unknown option --hosts-dir") || said.contains("names no extension") || cfg!(feature = "dev"), "{said}");
}
