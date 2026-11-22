//! `claktory-browser-host chrome-extension://<id>/`: what Chrome runs.
//! `--register [--replace]` and `--unregister`: what the app runs, after
//! the person agreed in its setup, to add or remove Chrome's manifest for
//! this helper. A Dev build also takes `--hosts-dir DIR` with them, for a
//! throwaway Chrome for Testing profile's NativeMessagingHosts.

use std::io::IsTerminal;
use std::process::ExitCode;

use claktory_browser_host::flavor::FLAVOR;
use claktory_browser_host::register::{self, Manifest};
use claktory_browser_host::relay::{self, Ended, Setup};
use claktory_browser_host::{frame, peer};

/// This build's version, as the build scripts stamp it (the app's own, since
/// the helper ships inside it); the package's otherwise.
const VERSION: &str = match option_env!("FACTORY_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

const USAGE: &str = "Chrome starts this helper for Claktory's browser extension; it isn't run by hand.";

fn fail(why: &str) -> ExitCode {
    // stderr is for diagnostics, never page data; stdout is framing only.
    eprintln!("claktory-browser-host: {why}");
    ExitCode::FAILURE
}

fn manage(args: &[String]) -> ExitCode {
    let Some(caller) = FLAVOR.caller() else { return fail("this build names no extension yet") };
    let mut replace = false;
    let mut file = None;
    let mut rest = args[1..].iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--replace" if args[0] == "--register" => replace = true,
            "--hosts-dir" if cfg!(feature = "dev") => {
                let Some(dir) = rest.next() else { return fail("--hosts-dir needs a folder") };
                file = Some(std::path::PathBuf::from(dir).join(format!("{}.json", FLAVOR.host)));
            }
            _ => return fail(&format!("unknown option {arg}")),
        }
    }
    let file = match file.map_or_else(|| FLAVOR.manifest(), Ok) {
        Ok(file) => file,
        Err(why) => return fail(&why),
    };
    let path = match register::helper_path() {
        Ok(path) => path,
        Err(why) => return fail(&why),
    };
    let manifest = Manifest { host: FLAVOR.host, caller: &caller, path: &path };
    let done = if args[0] == "--register" {
        register::register(&file, &manifest, replace).map(|()| format!("registered {}", file.display()))
    } else {
        register::unregister(&file, &manifest)
            .map(|removed| if removed { format!("removed {}", file.display()) } else { "nothing to remove".into() })
    };
    match done {
        Ok(said) => {
            println!("{said}");
            ExitCode::SUCCESS
        }
        Err(why) => fail(&why),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--register" | "--unregister") => return manage(&args),
        Some(caller) if caller.starts_with("chrome-extension://") => {}
        _ => return fail(USAGE),
    }
    // Chrome gives it pipes; a terminal means someone ran it by hand.
    if std::io::stdin().is_terminal() || std::io::stdout().is_terminal() {
        return fail(USAGE);
    }
    let caller = args[0].as_str();
    if FLAVOR.caller().as_deref() != Some(caller) {
        return fail("started for an extension that isn't Claktory's");
    }
    let requirement = peer::own().and_then(|own| {
        let exe = std::env::current_exe().and_then(|exe| exe.canonicalize()).map_err(|_| "this helper can't find itself".to_owned())?;
        FLAVOR.requirement(&own, &exe)
    });
    let requirement = match requirement {
        Ok(requirement) => requirement,
        Err(why) => {
            let _ = frame::write(
                &mut std::io::stdout().lock(),
                frame::Order::Native,
                &serde_json::json!({"type": "ended", "reason": "app_refused"}),
            );
            return fail(&why);
        }
    };
    let socket = match FLAVOR.socket() {
        Ok(socket) => socket,
        Err(why) => return fail(&why),
    };
    let setup = Setup { caller: FLAVOR.caller(), socket, requirement, version: VERSION.to_owned() };
    let ended = relay::run(caller, &setup, std::io::stdin(), std::io::stdout());
    match ended {
        ended if ended.clean() => ExitCode::SUCCESS,
        Ended::Refused(refusal) => fail(&format!("ended: {refusal}")),
        Ended::NoApp(why) | Ended::AppRefused(why) => fail(&why),
        other => fail(&format!("{other:?}")),
    }
}
