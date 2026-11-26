//! Every integration test of this crate, as one binary: each file here is
//! a module (run one with `cargo test -p factory-agent --test all <module>::`).
//! One binary links and starts far faster than one per file. A test that
//! needs a process of its own stays a file of its own in tests/ (tls.rs:
//! it checks rustls's process-wide default).

#[path = "../support/mod.rs"]
mod support;

mod accounts;
mod agent;
mod anthropic_auth;
mod anthropic_files;
mod anthropic_oauth;
mod anthropic_provider;
mod asking;
mod bash;
mod catalog;
mod clock;
mod codex_oauth;
mod codex_sse;
mod codex_websocket;
mod compaction;
mod credentials;
mod diff;
mod env_api_key_auth;
mod errors;
mod estimate;
mod extra_dirs;
mod glm_reasoning;
mod guard;
mod image_making;
mod images;
mod json_parse;
mod limits;
mod openai_provider;
mod openai_responses;
mod openrouter_oauth;
mod openrouter_provider;
mod reasoning;
mod refusal;
mod registry;
mod retry;
mod schema;
mod secrets;
mod sse;
mod stream;
mod system_notes;
mod tools;
mod types;
mod usage;
