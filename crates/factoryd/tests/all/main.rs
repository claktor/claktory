//! Every integration test of this crate, as one binary: each file here is
//! a module (run one with `cargo test -p factoryd --test all <module>::`).
//! One binary links and starts far faster than one per file. A test that
//! needs a process of its own stays a file of its own in tests/ (tls.rs:
//! it checks rustls's process-wide default).

#[path = "../common/mod.rs"]
mod common;

mod accounts;
mod agents;
mod app_data;
mod app_pins;
mod app_update;
mod app_views;
mod apps;
mod archive;
mod ask_each_time;
mod asks;
mod aws;
mod aws_stack;
mod aws_template;
mod build_caches;
mod changelog;
mod channel;
mod characters;
mod chats;
mod claktory_account;
mod cli;
mod cloudflare;
mod commands;
mod comments;
mod connectors;
mod database;
mod devices;
mod events;
mod explorer;
mod extensions;
mod factory_manual;
mod fallback;
mod folders;
mod highlight;
mod identity;
mod idle_compaction;
mod leads;
mod login;
mod map;
mod monitors;
mod move_app;
mod move_export;
mod move_import;
mod move_receive;
mod move_rivet;
mod notify;
mod onboarding;
mod origin;
mod owner_devices;
mod patches;
mod person;
mod pipe_link;
mod pipes;
mod pipes_bound;
mod pipes_quarantine;
mod pipes_review;
mod proposals;
mod protocol;
mod providers;
mod remote;
mod replies;
mod resources;
mod secrets_generate;
mod sign_ins;
mod speech;
mod suggest_talk;
mod supervisor;
mod swarm;
mod tasks;
mod tool_details;
mod traffic;
mod transcript_pages;
mod trees;
mod update;
mod uploads;
mod usage_report;
mod vault;
mod viewer;
mod workspace;
