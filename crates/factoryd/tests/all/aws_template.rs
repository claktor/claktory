//! The AWS connection's CloudFormation template (crates/factoryd/aws): its
//! function's own tests, run with node (skipped where node isn't
//! installed), and cfn-lint over the template where it is.

use std::path::Path;
use std::process::Command;

fn aws_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("aws")
}

/// Runs `program` with `args`, or None when it isn't installed.
fn run(program: &str, args: &[&str]) -> Option<std::process::Output> {
    match Command::new(program).args(args).current_dir(aws_dir()).output() {
        Ok(output) => Some(output),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("skipped: {program} isn't installed");
            None
        }
        Err(error) => panic!("{program}: {error}"),
    }
}

#[test]
fn the_templates_function_passes_its_own_tests() {
    let Some(output) = run("node", &["--test", "seal-test.js"]) else { return };
    assert!(output.status.success(), "{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
}

#[test]
fn the_template_lints_clean() {
    let Some(output) = run("cfn-lint", &["claktory-aws-v1.yaml"]) else { return };
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stdout));
}
