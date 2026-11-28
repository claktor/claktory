//! Checks that the visitor is a person. The agent acts for its person, so
//! it gets past one itself, as a person would: browser_screenshot shows the
//! check, and browser_act's click_at presses its box, picks its pictures or
//! holds its button. If it can't, it hands the check to the person
//! (browser_handoff kind "check", `super::handoff`), who gets past it in a
//! live view of that tab, and the browser is the agent's again straight
//! after. Nothing waits and nothing is tried again later.

use std::time::Duration;

/// How long a check gets to clear by itself on arrival: some pass on their
/// own in a few seconds (a browser check that runs and goes).
pub(super) const CLEARS: Duration = Duration::from_secs(10);

/// How long tests wait instead for a check to clear.
#[derive(Clone, Copy)]
pub(crate) struct Timing {
    pub clears: Duration,
}

impl Default for Timing {
    fn default() -> Timing {
        Timing { clears: CLEARS }
    }
}

/// Whether a check by `who` may pass on its own in a few seconds: a
/// page that checks the browser before it shows the site. A box or a
/// puzzle in the page waits for someone to act.
pub(crate) fn clears_by_itself(who: &str) -> bool {
    matches!(who, "cloudflare" | "google" | "datadome" | "human")
}

/// Who checks, in words: the check's maker, as `Page::check` names it.
fn by(who: &str) -> &'static str {
    match who {
        "cloudflare" => "Cloudflare's",
        "google" => "Google's",
        "datadome" => "DataDome's",
        "human" => "HUMAN's",
        "recaptcha" => "a reCAPTCHA",
        "hcaptcha" => "an hCaptcha",
        "arkose" => "an Arkose",
        _ => "a",
    }
}

/// What the agent is told when `site` shows a check by `who` that didn't
/// clear by itself.
pub(crate) fn found(site: &str, who: &str, person: &str) -> String {
    let site = if site.is_empty() { "The page" } else { site };
    format!(
        "{site} shows {} check that you're a person. Get past it yourself, as a person would: browser_screenshot shows it; browser_act's click_at presses its box, picks its pictures, holds its button (hold) or drags its slider or piece (to_x, to_y); then take another screenshot to see what it did. If you still can't get past it after two tries, hand it to {person} with browser_handoff kind \"check\": they get past it in a live view of this tab while you carry on elsewhere in a tab of your own, and you're back here once they answer.",
        by(who)
    )
}

/// Tests: checks get `clears` to clear by themselves.
#[doc(hidden)]
pub fn check_timing_for_test(factory: &crate::factory::Factory, clears: Duration) {
    *crate::locking::lock(&factory.errands.check_timing) = Timing { clears };
}
