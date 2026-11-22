//! Voice messages (#197): the speech model on this computer (`model`), and
//! transcribing what the person said before the agent gets it (`voice`).

pub(crate) mod dictation;
pub(crate) mod model;
pub(crate) mod voice;

pub use model::Artifact;
