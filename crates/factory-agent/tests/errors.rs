use std::error::Error as _;

use factory_agent::errors::{AgentError, AgentFailure, Error};

#[test]
fn the_failure_catalog_retains_display_messages_and_standard_error_behavior() {
    let examples = [
        (
            AgentFailure::CorruptCredentials { file: "auth.json".into() },
            "corrupt-credentials",
            "Corrupt credential file at auth.json. Delete it and sign in again.",
        ),
        (AgentFailure::WorkspaceEscape { path: "../secret".into() }, "workspace-escape", "Path escapes the workspace: ../secret"),
        (
            AgentFailure::ReadOnly { path: "/them/a.md".into() },
            "read-only",
            "Path is read only: /them/a.md. Change files only in your own folder.",
        ),
        (
            AgentFailure::InvalidEdit { path: "a.py".into(), reason: "replacement ranges overlap".into() },
            "invalid-edit",
            "Cannot edit a.py: replacement ranges overlap",
        ),
        (
            AgentFailure::BashWrappedInvocation { command: "bash -c".into() },
            "bash-wrapped-invocation",
            "command is \"bash -c\", not a script. Put the script text itself in command.",
        ),
    ];
    for (failure, kind, message) in examples {
        let cause = std::io::Error::other("original failure");
        let error = AgentError::with_cause(failure.clone(), cause);
        assert_eq!(error.failure, failure);
        assert_eq!(error.failure.kind(), kind);
        assert_eq!(error.to_string(), message);
        assert_eq!(error.source().map(ToString::to_string), Some("original failure".into()));
        let crate_error = Error::from(AgentError::new(failure.clone()));
        assert_eq!(crate_error.to_string(), message);
        assert_eq!(crate_error.failure(), Some(&failure));
    }
    let io: Error = std::io::Error::other("disk").into();
    assert_eq!(io.failure(), None);
    assert_eq!(io.to_string(), "disk");
}

#[test]
fn filesystem_errors_print_like_node() {
    use std::path::Path;

    use factory_agent::errors::FsError;

    let enoent = std::io::Error::from_raw_os_error(libc::ENOENT);
    assert_eq!(
        Error::fs(enoent, "open", Path::new("/w/missing.txt")).to_string(),
        "ENOENT: no such file or directory, open '/w/missing.txt'"
    );
    let eisdir = FsError::new(std::io::Error::from_raw_os_error(libc::EISDIR), "read", None, None);
    assert_eq!(eisdir.to_string(), "EISDIR: illegal operation on a directory, read");
    let rename = Error::fs_rename(std::io::Error::from_raw_os_error(libc::EISDIR), Path::new("/w/src/full.tmp"), Path::new("/w/src/full"));
    assert_eq!(rename.to_string(), "EISDIR: illegal operation on a directory, rename '/w/src/full.tmp' -> '/w/src/full'");
    let enotdir = Error::fs(std::io::Error::from_raw_os_error(libc::ENOTDIR), "realpath", Path::new("/w/a/x"));
    assert_eq!(enotdir.to_string(), "ENOTDIR: not a directory, realpath '/w/a/x'");
    let unknown = Error::fs(std::io::Error::new(std::io::ErrorKind::InvalidData, "bad bytes"), "open", Path::new("/w/x"));
    assert_eq!(unknown.to_string(), "UNKNOWN: bad bytes, open '/w/x'");
    let Error::Fs(error) = &unknown else { panic!("variant") };
    assert_eq!((error.code, error.syscall), ("UNKNOWN", "open"));
    assert_eq!(unknown.failure(), None);
    assert_eq!(Error::Aborted.to_string(), "This operation was aborted");
}
