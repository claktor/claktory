//! Site logins for agents' browsers: a login is an ordinary
//! secret, USERNAME and PASSWORD, bound to the one exact https origin
//! Chrome reported for the page when the agent asked (its sites, sealed as
//! `sites` seals them) and granted fill-only. Here: which login an agent
//! holds for an origin, which of the vault's fit one, and which is named
//! like it for another site (the card's warning, display only).

use super::{Vault, VaultError, connections, sites::Fillable};

/// A login's variables.
pub(crate) const USERNAME: &str = "USERNAME";
pub(crate) const PASSWORD: &str = "PASSWORD";

/// `origin`'s host's registrable domain in ASCII ("https://accounts.x.com"
/// → "x.com"), or the host itself when the public suffix list names none;
/// empty for anything but an https origin with a name.
pub(crate) fn site_of(origin: &str) -> String {
    let Some(host) = url::Url::parse(origin).ok().filter(|url| url.scheme() == "https").and_then(|url| url.host_str().map(str::to_owned)) else {
        return String::new();
    };
    psl::domain_str(&host).map_or(host.clone(), str::to_owned)
}

/// The name a new login for `site` gets: "X login" for x.com.
pub(crate) fn login_name(site: &str) -> String {
    let brand = site.split('.').next().unwrap_or(site);
    let mut letters = brand.chars();
    let capitalised: String = letters.next().map(|first| first.to_ascii_uppercase()).into_iter().chain(letters).collect();
    format!("{capitalised} login")
}

/// The letters a lookalike swaps in for ASCII ones: Cyrillic and Greek
/// that read as Latin, and digits that read as letters.
fn skeleton(text: &str) -> String {
    text.chars()
        .filter(|c| *c != '-')
        .map(|c| match c.to_lowercase().next().unwrap_or(c) {
            'а' | 'α' => 'a',
            'с' | 'ϲ' => 'c',
            'ԁ' => 'd',
            'е' | 'ε' => 'e',
            'ɡ' => 'g',
            'һ' => 'h',
            'і' | 'ι' | 'ı' | '1' | 'l' | 'ӏ' => 'l',
            'ј' => 'j',
            'к' | 'κ' => 'k',
            'м' => 'm',
            'п' | 'η' => 'n',
            'о' | 'ο' | '0' => 'o',
            'р' | 'ρ' => 'p',
            'ѕ' | '5' => 's',
            'т' | 'τ' => 't',
            'υ' | 'ս' => 'u',
            'ν' => 'v',
            'ѡ' | 'ω' => 'w',
            'х' | 'χ' => 'x',
            'у' | 'γ' => 'y',
            other => other,
        })
        .collect()
}

/// How many single letters to change, add or drop to make one word the
/// other.
fn distance(one: &str, other: &str) -> usize {
    let other: Vec<char> = other.chars().collect();
    let mut row: Vec<usize> = (0..=other.len()).collect();
    for (i, a) in one.chars().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, b) in other.iter().enumerate() {
            let here = row[j + 1];
            row[j + 1] = (previous + usize::from(a != *b)).min(row[j] + 1).min(here + 1);
            previous = here;
        }
    }
    row[other.len()]
}

/// Whether a page at `host`, of registrable domain `site`, reads like
/// `known`, another registrable domain: its name is a word of the host
/// (x-login.com, x.com.example.net), the same name under another ending
/// (x.co), the same letters in other scripts (xn--…), or one letter off
/// for a longer name (gooogle.com).
pub(crate) fn looks_like(host: &str, site: &str, known: &str) -> bool {
    if site == known || known.is_empty() {
        return false;
    }
    let brand = |domain: &str| domain.split('.').next().unwrap_or(domain).to_owned();
    let unicode = |domain: &str| idna::domain_to_unicode(domain).0;
    let known_brand = brand(known);
    let page_brand = brand(&unicode(site));
    if known_brand.is_empty() {
        return false;
    }
    if host.split(['.', '-']).any(|word| word == known_brand) {
        return true;
    }
    let (known_shape, page_shape) = (skeleton(&known_brand), skeleton(&page_brand));
    known_shape == page_shape || (known_brand.chars().count() >= 5 && distance(&known_shape, &page_shape) <= 1)
}

impl Vault {
    /// Whether `agent` holds a grant of `secret` its commands can use (not a
    /// fill-only one), whose MAC checks.
    pub(crate) fn holds_fully(&self, secret: &str, agent: &str) -> bool {
        let inner = self.lock();
        let Ok(key) = Self::grant_key(&inner) else { return false };
        Self::is_granted(&inner, &key, secret, agent).unwrap_or(false)
    }

    /// The login `agent` holds for exactly `origin`: a secret it holds (a
    /// full or fill-only grant whose MAC checks) whose sites hold it, that
    /// doesn't ask each time, and has a PASSWORD or USERNAME; the one
    /// changed last when there are several. None when it holds none; why
    /// not while the vault is locked.
    pub(crate) fn login_for(&self, agent: &str, origin: &str) -> Result<Option<Fillable>, String> {
        let inner = self.lock();
        let Some(db) = &inner.db else { return Ok(None) };
        let (Ok(key), Ok(vmk)) = (Self::grant_key(&inner), Self::open_vmk(&inner)) else {
            return Err("The vault is locked, so Claktory can't sign in: try again later.".to_owned());
        };
        let read = || -> Result<Option<Fillable>, VaultError> {
            let held: Vec<(String, String, String)> = crate::store::query(
                db,
                "SELECT secrets.id, secrets.name, secrets.kind FROM grants JOIN secrets ON secrets.id = grants.secret
                 WHERE grants.agent = ?1 AND secrets.ask = 0 AND secrets.sites != '' ORDER BY secrets.updated DESC, secrets.id",
                [agent],
                crate::store::row3,
            )?;
            for (id, name, kind) in held {
                if kind == connections::KIND_CONNECTION || Self::grant_of(&inner, &key, &id, agent)?.is_none() {
                    continue;
                }
                let sites = Self::sites_of(&inner, &key, &id)?;
                if !sites.iter().any(|site| site == origin) {
                    continue;
                }
                let values: Vec<super::Value> =
                    Self::values_of(db, &inner.vault_id, vmk, &id)?.into_iter().filter(|value| !value.file && !value.binary).collect();
                if values.iter().any(|value| value.variable == USERNAME || value.variable == PASSWORD) {
                    return Ok(Some(Fillable { id, name, sites, values }));
                }
            }
            Ok(None)
        };
        read().map_err(|error| error.to_string())
    }

    /// The vault's secrets whose sites hold exactly `origin` (their MAC
    /// checking), that `agent` doesn't hold: (id, name), changed last
    /// first. What "One of yours fits" offers.
    pub(crate) fn logins_at(&self, agent: &str, origin: &str) -> Vec<(String, String)> {
        let inner = self.lock();
        let (Ok(key), Ok(db)) = (Self::grant_key(&inner), Self::db(&inner)) else { return Vec::new() };
        let read = || -> Result<Vec<(String, String)>, VaultError> {
            let rows: Vec<(String, String, String)> = crate::store::query(
                db,
                "SELECT id, name, kind FROM secrets WHERE sites != '' AND ask = 0 ORDER BY updated DESC, id",
                [],
                crate::store::row3,
            )?;
            let mut found = Vec::new();
            for (id, name, kind) in rows {
                if kind == connections::KIND_CONNECTION || Self::grant_of(&inner, &key, &id, agent)?.is_some() {
                    continue;
                }
                if Self::sites_of(&inner, &key, &id)?.iter().any(|site| site == origin) {
                    found.push((id, name));
                }
            }
            Ok(found)
        };
        read().unwrap_or_else(|error| {
            tracing::warn!(%error, "could not read the vault's logins for a site");
            Vec::new()
        })
    }

    /// A secret of the vault's bound to another site that `origin` reads
    /// like (`looks_like`): its name and that site's registrable domain.
    /// None when there's none, or while the vault is locked.
    pub(crate) fn lookalike(&self, origin: &str) -> Option<(String, String)> {
        let host = url::Url::parse(origin).ok()?.host_str()?.to_owned();
        let site = site_of(origin);
        let inner = self.lock();
        let (Ok(key), Ok(db)) = (Self::grant_key(&inner), Self::db(&inner)) else { return None };
        let rows: Vec<(String, String)> =
            crate::store::query(db, "SELECT id, name FROM secrets WHERE sites != '' ORDER BY updated DESC, id", [], crate::store::row2).ok()?;
        for (id, name) in rows {
            let Ok(sites) = Self::sites_of(&inner, &key, &id) else { continue };
            if sites.iter().any(|known| site_of(known) == site) {
                // Bound to this very site too: it's this site's, not a lookalike's.
                continue;
            }
            if let Some(known) = sites.iter().map(|known| site_of(known)).find(|known| looks_like(&host, &site, known)) {
                return Some((name, known));
            }
        }
        None
    }
}

/// For tests: `agent`'s browser asks for its login to `origin`, as
/// browser_sign_in does on a page Chrome says is there, and the request
/// shows in its conversation.
pub fn ask_login_for_test(factory: &crate::factory::Factory, agent: &str, origin: &str) -> factory_protocol::v1::SecretRequest {
    let (request, _) = factory.vault.ask_login(agent, origin, "", None, factory_protocol::v1::SecretLoginOutcome::Unspecified).expect("asked");
    factory.vault_changed();
    request
}

/// For tests: `agent`'s latest login request for `origin`, as its card
/// shows it (what fits it, and what it looks like).
pub fn login_request_for_test(factory: &crate::factory::Factory, agent: &str, origin: &str) -> Option<factory_protocol::v1::SecretRequest> {
    let request = factory.vault.login_request(agent, origin).expect("read")?;
    Some(super::requests::fitted(factory, request))
}

/// For tests: what a sign-in with request `id`'s login did.
pub fn login_outcome_for_test(factory: &crate::factory::Factory, id: &str, outcome: factory_protocol::v1::SecretLoginOutcome) {
    let now = factory.vault.set_login_outcome(id, outcome).expect("kept");
    super::requests::show_everywhere(factory, now);
}

#[cfg(test)]
mod tests {
    use super::super::tests::{temp, value, vault};
    use super::*;

    #[test]
    fn a_logins_site_and_name_come_from_the_origin() {
        assert_eq!(site_of("https://accounts.x.com"), "x.com");
        assert_eq!(site_of("https://www.bbc.co.uk"), "bbc.co.uk");
        assert_eq!(site_of("http://x.com"), "");
        assert_eq!(login_name("x.com"), "X login");
        assert_eq!(login_name("linkedin.com"), "Linkedin login");
    }

    #[test]
    fn lookalikes_are_told_apart_from_the_site_itself() {
        let like = |origin: &str, known: &str| {
            let host = url::Url::parse(origin).unwrap().host_str().unwrap().to_owned();
            looks_like(&host, &site_of(origin), known)
        };
        assert!(like("https://x-login.com", "x.com"));
        assert!(like("https://x.com.evil.example", "x.com"));
        assert!(like("https://x.co", "x.com"));
        assert!(like("https://paypa1.com", "paypal.com"));
        assert!(like("https://gooogle.com", "google.com"));
        // аpple.com in Cyrillic а, as Chrome reports it.
        assert!(like("https://xn--pple-43d.com", "apple.com"));
        assert!(!like("https://accounts.x.com", "x.com"), "the same site");
        assert!(!like("https://box.com", "x.com"));
        assert!(!like("https://github.com", "gitlab.com"));
    }

    #[test]
    fn an_agent_holds_a_login_only_for_its_exact_origin() {
        let dir = temp();
        let vault = vault(&dir);
        vault.create(Some("correct horse battery"), true).expect("create").expect("recovery");
        let values = [value(USERNAME, "matt@example.com"), value(PASSWORD, "correct horse battery staple")];
        let id = vault.create_secret("X login", "", &values, "").expect("secret");
        vault.set_sites(&id, &["https://x.com".to_owned()]).expect("bound");
        assert!(vault.login_for("yukako", "https://x.com").expect("read").is_none(), "not held");
        assert_eq!(vault.logins_at("yukako", "https://x.com"), [(id.clone(), "X login".to_owned())]);
        assert!(vault.logins_at("yukako", "https://www.x.com").is_empty(), "never a subdomain");
        vault.grant_fill_only(&id, "yukako", "").expect("grant");
        assert_eq!(vault.login_for("yukako", "https://x.com").expect("read").expect("held").id, id);
        assert!(vault.login_for("yukako", "https://x-login.com").expect("read").is_none());
        assert!(vault.logins_at("yukako", "https://x.com").is_empty(), "held already");
        assert_eq!(vault.lookalike("https://x-login.com"), Some(("X login".to_owned(), "x.com".to_owned())));
        assert_eq!(vault.lookalike("https://x.com"), None);
        assert_eq!(vault.lookalike("https://mobile.x.com"), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_login_shows_no_end_of_its_password_and_a_short_username_only_matters_for_commands() {
        let dir = temp();
        let vault = vault(&dir);
        vault.create(Some("correct horse battery"), true).expect("create").expect("recovery");
        let values = [value(USERNAME, "qa-user"), value(PASSWORD, "correct horse battery staple")];
        let id = vault.create_secret("X login", "", &values, "").expect("secret");
        vault.set_sites(&id, &["https://x.com".to_owned()]).expect("bound");
        vault.grant_fill_only(&id, "yukako", "").expect("grant");
        let shown = || vault.state(|_, _| false).secrets.into_iter().find(|secret| secret.id == id).expect("the login");
        let login = shown();
        let hints: Vec<(&str, &str)> = login.variables.iter().map(|variable| (variable.name.as_str(), variable.hint.as_str())).collect();
        assert_eq!(hints, [(USERNAME, ""), (PASSWORD, "")]);
        assert!(login.warnings.is_empty(), "typed only into its site: {:?}", login.warnings);
        vault.grant(&id, "bo", "").expect("a full grant");
        assert_eq!(shown().warnings, ["USERNAME is too short to hide in agents' output."]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_secret_with_sites_shows_a_hint_for_its_username_alone_whatever_its_fields_are_called() {
        let dir = temp();
        let vault = vault(&dir);
        vault.create(Some("correct horse battery"), true).expect("create").expect("recovery");
        let values = [value(USERNAME, "matt@example.com"), value("MEMORABLE_WORD", "aubergine-sunset")];
        let id = vault.create_secret("Bank login", "", &values, "").expect("secret");
        let hints = || -> Vec<(String, String)> {
            let secret = vault.state(|_, _| false).secrets.into_iter().find(|secret| secret.id == id).expect("the login");
            secret.variables.into_iter().map(|variable| (variable.name, variable.hint)).collect()
        };
        assert_eq!(hints(), [(USERNAME.to_owned(), ".com".to_owned()), ("MEMORABLE_WORD".to_owned(), "nset".to_owned())]);
        vault.set_sites(&id, &["https://bank.example".to_owned()]).expect("bound");
        assert_eq!(hints(), [(USERNAME.to_owned(), ".com".to_owned()), ("MEMORABLE_WORD".to_owned(), String::new())]);
        vault.update_secret(&id, "Bank login", "", Some(&[value(USERNAME, "matt@example.com"), value("MEMORABLE_WORD", "courgette-dawn")])).expect("new values");
        assert_eq!(hints(), [(USERNAME.to_owned(), ".com".to_owned()), ("MEMORABLE_WORD".to_owned(), String::new())]);
        let db = rusqlite::Connection::open(dir.join("vault/vault.db")).expect("the vault's file");
        let kept: Vec<String> = db.prepare("SELECT hint FROM variables").unwrap().query_map([], |row| row.get(0)).unwrap().map(Result::unwrap).collect();
        assert!(!kept.iter().any(|hint| hint == "dawn" || hint == "nset"), "{kept:?}");
        let _ = std::fs::remove_dir_all(dir);
    }
}
