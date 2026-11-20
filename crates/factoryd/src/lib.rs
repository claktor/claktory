//! factoryd: the Factory backend. Clients connect over WebSocket and speak the
//! protocol defined in `proto/factory/v1/factory.proto`.

mod account;
mod annotations;
mod app_archive;
pub mod apps;
mod arrange;
pub mod channel;
mod characters;
mod commands;
pub mod config;
mod connection;
#[doc(hidden)]
pub mod connectors;
mod encoding;
mod explorer;
mod extensions;
pub mod factory;
mod forward;
pub mod home;
mod idle;
pub mod launch_agent;
mod login;
mod manual;
mod map;
pub mod models;
mod network;
mod patch;
mod person;
mod proposals;
pub mod remote;
mod runner;
mod runs;
mod store;
pub mod terminal_qr;
pub mod transcript;
mod trees;
pub mod update;
mod uploads;
mod usage;
mod usage_report;
pub mod vault;
mod waits;

pub use connection::{ConnectionError, serve};
pub use factory::Factory;
pub use runner::set_filter_executable;
