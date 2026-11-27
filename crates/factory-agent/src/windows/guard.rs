//! What agents' commands may not start on Windows, as `tools::asking` says
//! elsewhere ("Elevation guard"): a program whose job is to run something
//! as an administrator, which puts up the UAC prompt a habitual click
//! would approve.
//!
//! Best effort, as the spec says: any program can ask Windows for UAC
//! (ShellExecute's "runas", PowerShell's `Start-Process -Verb RunAs`), and
//! the prompt itself is drawn by consent.exe, outside the command's job.
//! What's stopped here is the known programs that do nothing else.

/// The program the file `path` is, by the name the agent is told, when it's
/// one that asks to act as an administrator.
pub fn asking(path: &str) -> Option<&'static str> {
    let file = path.rsplit(['\\', '/']).next().unwrap_or_default().to_ascii_lowercase();
    let name = file.strip_suffix(".exe").unwrap_or(&file);
    match name {
        // Windows 11's own sudo, and the popular one.
        "sudo" => Some("sudo"),
        "gsudo" => Some("gsudo"),
        "runas" => Some("runas"),
        _ => None,
    }
}

/// A command whose text asks PowerShell to run something as an
/// administrator (`Start-Process … -Verb RunAs`), refused before it starts
/// the program it names, for the agent's message.
pub fn asking_in_text(command: &str) -> Option<&'static str> {
    let text = command.to_ascii_lowercase();
    let squeezed: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    (squeezed.contains("-verb runas") || squeezed.contains("-verb:runas")).then_some("Start-Process -Verb RunAs")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_known_elevators_are_named_and_others_are_not() {
        assert_eq!(asking(r"C:\Windows\System32\sudo.exe"), Some("sudo"));
        assert_eq!(asking(r"C:\Users\fred\scoop\shims\GSUDO.EXE"), Some("gsudo"));
        assert_eq!(asking(r"C:\Windows\System32\runas.exe"), Some("runas"));
        assert_eq!(asking(r"C:\Program Files\Git\usr\bin\bash.exe"), None);
        assert_eq!(asking(""), None);
    }

    #[test]
    fn powershell_asking_for_runas_is_named_in_the_text() {
        assert_eq!(asking_in_text("powershell -c 'Start-Process cmd -Verb RunAs'"), Some("Start-Process -Verb RunAs"));
        assert_eq!(asking_in_text("pwsh -c \"start-process x -verb:RUNAS\""), Some("Start-Process -Verb RunAs"));
        assert_eq!(asking_in_text("Start-Process x  -Verb\tRunAs"), Some("Start-Process -Verb RunAs"));
        assert_eq!(asking_in_text("echo -verbose run as"), None);
    }
}
