//! The test harness itself: a test's background tasks end
//! with its folder, rather than outlive it.

use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::common::{end_tasks_under, ps_runs, temp_dir};

#[test]
fn a_tests_background_tasks_end_with_its_folder() {
    if !ps_runs() {
        eprintln!("skipped: ps can't start here, and tasks are found with it");
        return;
    }
    let dir = temp_dir();
    let tasks = dir.join("home/agents/builder/tasks");
    std::fs::create_dir_all(&tasks).expect("tasks");
    // As factory-agent's bash starts one: its own group, led by a shell
    // naming its status file; here with a second process in the group.
    let mut task = Command::new("bash")
        .args(["--noprofile", "--norc", "-c", "sleep 60 & sleep 60", "factory-task", "loop"])
        .arg(tasks.join("1.status"))
        .stdin(Stdio::null())
        .process_group(0)
        .spawn()
        .expect("a task");
    let group = task.id();
    // Another folder's task, which isn't touched.
    let other = temp_dir();
    let mut theirs = Command::new("bash")
        .args(["--noprofile", "--norc", "-c", "sleep 60", "factory-task", "theirs"])
        .arg(other.join("home/agents/a/tasks/1.status"))
        .stdin(Stdio::null())
        .process_group(0)
        .spawn()
        .expect("their task");

    end_tasks_under(&dir);
    let started = Instant::now();
    while task.try_wait().expect("wait").is_none() {
        assert!(started.elapsed() < Duration::from_secs(10), "the task still runs");
        std::thread::sleep(Duration::from_millis(20));
    }
    let left = Command::new("pgrep").args(["-g", &group.to_string()]).output().expect("pgrep");
    assert!(left.stdout.is_empty(), "its group still has {}", String::from_utf8_lossy(&left.stdout));
    assert!(theirs.try_wait().expect("wait").is_none(), "another folder's task runs on");
    let _ = theirs.kill();
    let _ = theirs.wait();
}
