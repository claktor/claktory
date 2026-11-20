//! Every integration test of this crate, as one binary: each file here is
//! a module (run one with `cargo test -p factoryd --test all <module>::`).
//! One binary links and starts far faster than one per file. A test that
//! needs a process of its own stays a file of its own in tests/ (tls.rs:
//! it checks rustls's process-wide default).

#[path = "../common/mod.rs"]
mod common;

mod accounts;
mod agents;
mod app_update;
mod archive;
mod build_caches;
mod changelog;
mod channel;
mod characters;
mod claktory_account;
mod commands;
mod comments;
mod connectors;
mod database;
mod events;
mod explorer;
mod extensions;
mod factory_manual;
mod folders;
mod highlight;
mod identity;
mod idle_compaction;
mod login;
mod map;
mod monitors;
mod origin;
mod patches;
mod proposals;
mod protocol;
mod providers;
mod remote;
mod resources;
mod supervisor;
mod tasks;
mod tool_details;
mod traffic;
mod transcript_pages;
mod trees;
mod update;
mod uploads;
mod vault;
mod viewer;
mod workspace;
