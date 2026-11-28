//! Every integration test of this crate, as one binary: each file here is
//! a module (run one with `cargo test -p factoryd --test all <module>::`).
//! One binary links and starts far faster than one per file. A test that
//! needs a process of its own stays a file of its own in tests/ (tls.rs:
//! it checks rustls's process-wide default).

#[path = "../common/mod.rs"]
mod common;

mod accounts;
mod admin_conversations;
mod agent_safety;
mod agents;
mod app_access;
mod app_files;
mod app_data;
// Not on Windows: the user-apps worker is reached over a Unix socket.
#[cfg(unix)]
mod app_grants;
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
mod browser_image_gate;
mod build_caches;
mod card_thread;
mod card_update;
mod changelog;
mod channel;
mod characters;
mod chats;
mod checklist_replies;
mod checklist_stale;
mod checklist_waits;
mod claktory_account;
mod cli;
mod cloudflare;
// A card doesn't run a command on Windows yet; it says so.
#[cfg(unix)]
mod command_cards;
mod commands;
mod comments;
mod connectors;
mod credit;
mod database;
mod device_allow;
mod devices;
mod diagnostics;
mod engine;
mod events;
mod explorer;
mod extensions;
mod factory_manual;
mod find;
mod fallback;
mod focus_mode;
mod folders;
mod fork;
mod given_up;
mod grok;
// Not on Windows: it ends tasks by their process groups; Windows' jobs are `common::cleanup`'s.
#[cfg(unix)]
mod harness;
mod hidden_characters;
mod highlight;
mod history;
mod identity;
mod ink;
mod ink_layers;
mod idle_compaction;
mod image_files;
mod image_generate;
mod keep_awake;
mod leads;
mod login;
mod logins;
mod map;
mod model_upgrades;
mod models_ext;
mod monitors;
// Moving a Claktory isn't on Windows yet (move_run::NOT_ON_WINDOWS).
#[cfg(unix)]
mod move_app;
// Not on Windows: moving isn't on Windows yet.
#[cfg(unix)]
mod move_export;
// Not on Windows: moving isn't on Windows yet.
#[cfg(unix)]
mod move_import;
// Not on Windows: moving isn't on Windows yet.
#[cfg(unix)]
mod move_receive;
#[cfg(unix)]
mod move_rivet;
mod needs_you;
mod no_pipe_mark;
mod notify;
mod onboarding;
mod openrouter_models;
mod origin;
mod owner_devices;
mod paced;
mod pairing_withdraw;
mod panel_reads;
mod patches;
mod person;
mod pins;
mod pipe_approval_card;
mod pipe_cards;
mod pipe_groups;
mod pipe_link;
mod pipe_trust_card;
mod pipe_websites;
mod pipes;
mod pipes_bound;
mod pipes_curator;
mod pipes_identity;
mod pipes_lead;
mod pipes_liaison;
mod pipes_quarantine;
mod pipes_report_types;
mod pipes_reports;
mod pipes_review;
mod pipes_sealed;
mod pipes_senders;
mod pipes_spend;
mod presentation;
mod previews;
mod project_brief;
mod project_order;
mod projects_sidebar;
mod proposal_grants;
mod proposals;
mod provenance;
mod protected;
mod protocol;
mod provider_models;
mod provider_outages;
mod provider_waits;
mod providers;
mod push_fcm;
mod qa_provider;
mod quiet;
mod quiet_hours;
mod remote;
mod replies;
mod report_reply;
mod resume;
mod review_items;
mod resources;
mod rivet_actions;
mod rivet_cards;
mod save_as;
mod seals;
mod secret_cards;
mod secrets_generate;
mod secrets_import;
mod settings;
mod sign_ins;
mod sketches;
mod slow_reads;
// Not on Windows: its helper is stood in for by shell scripts.
#[cfg(unix)]
mod speech;
mod start_over;
mod suggest_talk;
mod supervisor;
mod swarm;
mod tasks;
mod tool_details;
mod traffic;
mod transcript_load;
mod transcript_save;
mod transcript_pages;
mod trees;
mod unload;
mod update;
mod uploads;
mod urgent_alerts;
mod usage_report;
mod usage_switch;
mod usage_waits;
mod vault;
mod vault_running;
mod viewer;
mod windows_app;
mod withdraw;
mod workspace;
