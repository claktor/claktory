//! A file that takes long to read (a video's preview can take minutes)
//! doesn't hold up its connection: pings are answered, and other requests,
//! and the read is answered when it's done.

#![cfg(unix)]

use factory_protocol::v1::{self, ClientMessage, client_message, server_message::Body};

use crate::common::{Client, start, temp_dir};

fn read(path: &std::path::Path) -> client_message::Body {
    client_message::Body::ReadFile(v1::ReadFile { path: path.display().to_string(), ..Default::default() })
}

fn content(body: Body) -> v1::FileContent {
    match body {
        Body::FileContent(content) => content,
        other => panic!("expected file content, got {other:?}"),
    }
}

/// Opens the pipe for writing as the test ends, however it ends, so a read
/// still waiting on it (the test failed) ends too and the test doesn't hang.
struct Unblock(std::path::PathBuf);

impl Drop for Unblock {
    fn drop(&mut self) {
        use std::os::unix::fs::OpenOptionsExt;
        let _ = std::fs::OpenOptions::new().write(true).custom_flags(libc::O_NONBLOCK).open(&self.0);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_slow_read_holds_up_neither_pings_nor_other_requests() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, _, _) = Client::connect(addr).await;

    // A named pipe: reading it waits until something writes to it.
    let slow = dir.join("slow");
    let made = std::process::Command::new("mkfifo").arg(&slow).status().expect("mkfifo");
    assert!(made.success());
    let _unblock = Unblock(slow.clone());
    let quick = dir.join("quick.txt");
    std::fs::write(&quick, "quick").expect("write");

    client.send(ClientMessage { id: 900, body: Some(read(&slow)) }).await;
    let meanwhile = client.ping(b"still there?").await;
    assert!(meanwhile.iter().all(|message| message.reply_to != 900), "the slow read isn't done yet");

    let second = client.request(read(&quick)).await;
    assert_eq!(content(second.body).data, b"quick");

    // Written now, the slow read is answered.
    let writer = tokio::task::spawn_blocking(move || std::fs::write(&slow, "slow at last"));
    let answer = loop {
        let message = client.receive().await;
        if message.reply_to == 900 {
            break message;
        }
    };
    writer.await.expect("writer").expect("write the pipe");
    let answer = content(answer.body.expect("body"));
    assert_eq!((answer.error.as_str(), answer.data.as_slice()), ("", b"slow at last".as_slice()));
}
