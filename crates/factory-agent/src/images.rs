//! Images an agent's model makes: what's asked, what comes back, and
//! why one couldn't be made. Each provider that makes images has its own
//! request (`Provider::images`); phase 1 is ChatGPT's subscription
//! (`providers::openai_codex::images`).

use futures_util::future::BoxFuture;

use crate::auth::RequestAuth;

/// What the person's agent asked for: one image.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImageRequest {
    pub prompt: String,
    /// "1024x1024", "1536x1024", "1024x1536" or "auto"; None leaves it to
    /// the provider.
    pub size: Option<String>,
    /// "low", "medium", "high" or "auto"; None leaves it to the provider.
    pub quality: Option<String>,
    /// A transparent background, where the model can make one.
    pub transparent: bool,
}

/// An image made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MadeImage {
    /// The image file's bytes, as the provider sent them (PNG unless asked
    /// otherwise).
    pub bytes: Vec<u8>,
    /// The image model that made it, such as "gpt-image-2".
    pub model: String,
}

/// Why no image was made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageFailure {
    /// The provider makes no images, or not on this sign-in.
    NotOffered,
    /// Signed out of the provider, or its sign-ins are locked away.
    SignedOut,
    /// The plan's image limit is reached, until then (ms since 1970) if
    /// the provider said.
    Limit { resets_at: Option<u64> },
    /// The plan doesn't include images (ChatGPT Free).
    NotOnPlan,
    /// The provider's content rules declined it: the description
    /// (`output` false) or the image it made (`output` true).
    Declined { output: bool },
    /// The provider couldn't be reached, or didn't answer in time.
    Unreachable(String),
    /// Anything else the provider said, as it said it, in a sentence.
    Refused(String),
}

/// A provider's image request, on one of its sign-ins: its base url and
/// auth, as a usage read gets them.
pub type GenerateFn = fn(base_url: String, auth: RequestAuth, request: ImageRequest) -> BoxFuture<'static, Result<MadeImage, ImageFailure>>;
