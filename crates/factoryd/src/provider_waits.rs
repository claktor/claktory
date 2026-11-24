//! Agents and chats waiting on a provider's usage limit or credit, grouped
//! by provider (#343 U6, Workspace.provider_waits): one Needs you item each,
//! which notifies when it appears (`crate::notify`), so the person hears that
//! their agents stopped without having to look.

use factory_protocol::v1;

use crate::factory::Inner;

/// How many waiters a sentence names before it counts the rest.
const NAMED: usize = 2;

/// The providers waited on, in the order their first waiter is listed:
/// agents in `agents`' order, then chats in `chats`' order. Only waits
/// another provider could carry (`UsageWait.switchable`): a wait to sign in
/// or for the secrets to unlock isn't one.
pub(crate) fn view(inner: &Inner, agents: &[v1::Agent], chats: &[v1::Chat]) -> Vec<v1::ProviderWait> {
    let mut waits: Vec<v1::ProviderWait> = Vec::new();
    let mut names: Vec<Vec<String>> = Vec::new();
    let waiters = agents
        .iter()
        .map(|agent| (&agent.id, &agent.model, Some(&agent.name)))
        .chain(chats.iter().map(|chat| (&chat.id, &chat.model, None)));
    for (id, model, name) in waiters {
        let Some(wait) = inner.agents.get(id).and_then(|runtime| runtime.usage_wait.as_ref()).filter(|wait| wait.switchable) else {
            continue;
        };
        // Under the provider it waits on; one moved to another provider's
        // model meanwhile waits on nothing there, and is woken to try it.
        let provider = model.split_once('/').map_or(model.as_str(), |(provider, _)| provider);
        if wait.provider != provider {
            continue;
        }
        let index = match waits.iter().position(|known| known.provider_id == provider) {
            Some(index) => index,
            None => {
                let known = inner.providers.iter().find(|known| known.id == provider);
                let provider_name = known.map_or(provider, |known| known.name.as_str());
                waits.push(v1::ProviderWait {
                    provider_id: provider.to_owned(),
                    provider_name: factory_agent::accounts::account_noun(provider, provider_name),
                    credit: true,
                    ..v1::ProviderWait::default()
                });
                names.push(Vec::new());
                waits.len() - 1
            }
        };
        let entry = &mut waits[index];
        // A limit that resets by itself: the soonest any of them asks again.
        if wait.until_ms > 0 {
            entry.credit = false;
            entry.until_ms = if entry.until_ms == 0 { wait.until_ms } else { entry.until_ms.min(wait.until_ms) };
        }
        match name {
            Some(name) => {
                entry.agent_ids.push(id.clone());
                names[index].push(name.clone());
            }
            None => entry.chat_ids.push(id.clone()),
        }
    }
    for (wait, names) in waits.iter_mut().zip(names) {
        (wait.title, wait.text) = words(wait, &names);
    }
    waits
}

/// The item's title and its sentence: who waits, and until when.
fn words(wait: &v1::ProviderWait, names: &[String]) -> (String, String) {
    let noun = &wait.provider_name;
    let count = names.len() + wait.chat_ids.len();
    let who = who(names, wait.chat_ids.len());
    let verb = if count == 1 { "waits" } else { "wait" };
    if wait.credit {
        (format!("{noun} is out of credit"), format!("{who} {verb} until you add credit or another {noun} account."))
    } else {
        let at = u64::try_from(wait.until_ms).unwrap_or(0);
        let at = factory_agent::accounts::local_time(at).unwrap_or_else(|| factory_agent::accounts::when(at));
        (format!("{noun} usage limit reached"), format!("{who} {verb} until it resets at {at}."))
    }
}

/// "Rivet", "Rivet and Mike", "Rivet, Mike and 2 more", "Rivet and a chat",
/// "2 chats".
fn who(names: &[String], chats: usize) -> String {
    let mut parts: Vec<String> = names.iter().take(NAMED).cloned().collect();
    let rest = names.len().saturating_sub(NAMED);
    match (rest, chats) {
        (0, 0) => {}
        (0, 1) if !parts.is_empty() => parts.push("a chat".to_owned()),
        (0, 1) => parts.push("A chat".to_owned()),
        (0, chats) => parts.push(format!("{chats} chats")),
        (rest, chats) => parts.push(format!("{} more", rest + chats)),
    }
    match parts.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [first @ .., last] => format!("{} and {last}", first.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::{who, words};
    use factory_protocol::v1;

    #[test]
    fn credit_says_what_ends_it() {
        let wait = v1::ProviderWait {
            provider_name: "OpenRouter".into(),
            credit: true,
            agent_ids: vec!["rivet".into()],
            chat_ids: vec!["chat-1".into()],
            ..v1::ProviderWait::default()
        };
        assert_eq!(
            words(&wait, &["Rivet".to_owned()]),
            (
                "OpenRouter is out of credit".to_owned(),
                "Rivet and a chat wait until you add credit or another OpenRouter account.".to_owned()
            )
        );
    }

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn who_names_two_and_counts_the_rest() {
        assert_eq!(who(&names(&["Rivet"]), 0), "Rivet");
        assert_eq!(who(&names(&["Rivet", "Mike"]), 0), "Rivet and Mike");
        assert_eq!(who(&names(&["Rivet", "Mike", "Ana", "Lea"]), 0), "Rivet, Mike and 2 more");
        assert_eq!(who(&names(&["Rivet", "Mike", "Ana"]), 2), "Rivet, Mike and 3 more");
        assert_eq!(who(&names(&["Rivet"]), 1), "Rivet and a chat");
        assert_eq!(who(&names(&["Rivet", "Mike"]), 2), "Rivet, Mike and 2 chats");
        assert_eq!(who(&[], 1), "A chat");
        assert_eq!(who(&[], 3), "3 chats");
    }
}
