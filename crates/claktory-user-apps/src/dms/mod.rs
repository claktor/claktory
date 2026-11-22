//! DMs, the first user-level app (dms.md in the research folder): two
//! people's messages, kept at the account so they reach every device with
//! every Claktory off. Its fields are sealed at rest (`keys`), with keys
//! the account service never holds.

pub mod agents;
pub mod bodies;
pub mod contacts;
pub mod groups;
pub mod handoffs;
pub mod identity;
pub mod keys;
pub mod leaves;
pub mod messages;
pub mod reports;
pub mod review;
pub mod sync;
