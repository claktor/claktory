//! factoryd: the Factory backend. Clients connect over WebSocket and speak the
//! protocol defined in `proto/factory/v1/factory.proto`.

mod account;
mod answered_on;
mod annotations;
mod app_archive;
pub mod apps;
mod arrange;
mod arrange_undo;
pub mod browser;
// On Windows only what the browser's host reads of it: its wire and the
// input lease (the runner itself runs on Linux).
#[cfg_attr(windows, path = "windows/browser_runner.rs")]
pub mod browser_runner;
pub mod browser_host;
mod calls;
mod card_actions;
mod command_cards;
mod catalog;
pub mod channel;
mod characters;
mod chats;
pub mod cli;
mod commands;
pub mod config;
mod connection;
mod credit;
#[doc(hidden)]
pub mod connectors;
mod decisions;
pub mod diagnostics;
pub mod disk;
#[doc(hidden)]
pub mod dm_device;
mod dm_hello;
mod dm_link;
mod encoding;
pub mod errands;
mod explorer;
mod explorer_watch;
mod file_watch;
pub mod log_file;
mod browser_switches;
mod extensions;
pub mod factory;
mod find;
mod focus_mode;
pub mod fork;
mod forward;
mod given_up;
pub mod home;
mod idle;
mod ink;
mod iroh_link;
pub mod install;
mod keep_awake;
/// Keeping a laptop's agents working with its lid closed (#371); public
/// for tests' stand-in helper.
pub mod lid_closed;
pub mod launch_agent;
mod locking;
mod login;
mod manual;
mod history;
mod map;
pub mod models;
pub mod move_bundle;
pub mod move_export;
pub mod move_import;
mod move_report;
pub mod move_run;
pub mod move_source;
pub mod move_wire;
mod needs_you;
mod network;
pub mod notify;
mod onboarding;
pub mod open_files;
mod patch;
mod person;
mod pins;
pub mod pipe_link;
pub mod pipes;
mod presentation;
mod previews;
mod proposal_grants;
mod proposals;
pub mod protected;
mod provider_waits;
pub mod remote;
mod repeats;
#[doc(hidden)]
pub mod review_watch;
mod rivet_reach;
mod runner;
mod runs;
mod service;
mod sketch;
pub mod speech;
#[cfg(feature = "qa-lies")]
pub mod qa_lies;
mod store;
pub mod terminal_qr;
pub mod transcript;
mod trees;
mod unload;
pub mod uninstall;
pub mod update;
mod upgrades;
mod uploads;
mod usage;
mod usage_report;
mod usage_switch;
pub mod user_apps;
pub mod vault;
mod waits;
mod watch;
// Where Claktory lives on Windows and what the backend sets up there
// first (Jotaro's; main.rs reaches it).
#[cfg(windows)]
pub mod windows;

pub use account::analytics::{backend_started, backend_stopped};
/// For tests: anchors a test's own account service to roots of its own.
#[doc(hidden)]
pub use account::anchor::anchor_for_tests as anchor_account_for_tests;
/// For tests: a Factory done with, whose home another now uses.
#[doc(hidden)]
pub use account::stop_for_tests as stop_account_for_tests;
/// For tests: Execute cards' runs left going as if Claktory had died.
#[doc(hidden)]
pub use command_cards::abandon_runs_for_test;
pub use connection::{ConnectionError, serve};
/// For tests: one hourly Team health check, as if at a given time.
#[doc(hidden)]
pub use extensions::swarm::check_for_test as swarm_check_for_test;
pub use factory::Factory;
/// For tests: History's events counted ahead, as a while after a start,
/// and how many its kept counts hold.
#[doc(hidden)]
pub use history::{counted_for_test as history_counted_for_test, warm_for_test as history_warm_for_test};
pub use runner::set_filter_executable;
