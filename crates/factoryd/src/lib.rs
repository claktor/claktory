//! factoryd: the Factory backend. Clients connect over WebSocket and speak the
//! protocol defined in `proto/factory/v1/factory.proto`.

mod account;
mod annotations;
mod app_archive;
pub mod apps;
mod arrange;
pub mod browser;
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
mod file_watch;
pub mod log_file;
mod extensions;
pub mod factory;
mod find;
pub mod fork;
mod forward;
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
pub mod pipe_link;
pub mod pipes;
mod presentation;
mod previews;
mod proposal_grants;
mod proposals;
mod provider_waits;
pub mod remote;
mod repeats;
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

pub use account::analytics::{backend_started, backend_stopped};
/// For tests: anchors a test's own account service to roots of its own.
#[doc(hidden)]
pub use account::anchor::anchor_for_tests as anchor_account_for_tests;
/// For tests: Execute cards' runs left going as if Claktory had died.
#[doc(hidden)]
pub use command_cards::abandon_runs_for_test;
pub use connection::{ConnectionError, serve};
/// For tests: one hourly Team health check, as if at a given time.
#[doc(hidden)]
pub use extensions::swarm::check_for_test as swarm_check_for_test;
pub use factory::Factory;
pub use runner::set_filter_executable;
