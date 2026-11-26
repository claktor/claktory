//! Where a secret may be filled in Claktory's browser (#435 W6,
//! agent-browser.md §5.3, Fugo 40552): its sites, exact https origins as
//! `browser::origin` spells them, never a pattern or a domain ending, kept
//! with a MAC from the grants' key so editing vault.db can't bind a secret
//! to another site; the values a fill gets, for an agent that holds the
//! secret (a full grant or a fill-only one); every fill logged in `uses`;
//! and the one account Claktory's browser signed in with at each identity
//! provider.

use rusqlite::{Connection, OptionalExtension, params};

use super::{Grant, Inner, Value, Vault, VaultError, aad, connections, crypto, now_ms};
use crate::browser::origin;
use factory_protocol::v1;

/// The most sites one secret may be bound to.
pub(crate) const MOST_SITES: usize = 20;

/// What a secret's sites' MAC covers.
fn message(vault_id: &str, secret: &str, sites: &str) -> Vec<u8> {
    aad(&["sites", vault_id, secret, sites])
}

/// `given`, each spelled as Chrome spells an origin, in order, without
/// repeats; or why one can't be a secret's site, in words for the person.
pub(crate) fn checked(given: &[String]) -> Result<Vec<String>, VaultError> {
    let mut sites: Vec<String> = Vec::new();
    for site in given.iter().map(|site| site.trim()).filter(|site| !site.is_empty()) {
        let canonical = origin::canonical(site).map_err(|why| VaultError::Invalid(format!("{site}: {why}")))?;
        if !sites.contains(&canonical) {
            sites.push(canonical);
        }
    }
    if sites.len() > MOST_SITES {
        return Err(VaultError::Invalid(format!("A secret can be filled into {MOST_SITES} sites at most.")));
    }
    Ok(sites)
}

/// A secret's values, for one fill into its sites: none leaves the backend
/// except into the field the browser's host checked.
pub(crate) struct Fillable {
    pub id: String,
    pub name: String,
    /// Exact https origins, as checked.
    pub sites: Vec<String>,
    /// Its variables that are text, not files.
    pub values: Vec<Value>,
}

impl std::fmt::Debug for Fillable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Fillable {{ name: {:?}, sites: {:?}, values: {:?} }}", self.name, self.sites, self.values)
    }
}

impl Vault {
    /// The secret's sites as kept, if their MAC checks: `Ok(None)` when it
    /// doesn't (changed outside Claktory, so none count).
    fn sites_checked(inner: &Inner, key: &[u8; 32], db: &Connection, id: &str) -> Result<Option<Vec<String>>, VaultError> {
        let (sites, mac): (String, Vec<u8>) = db.query_row("SELECT sites, sites_mac FROM secrets WHERE id = ?1", [id], crate::store::row2)?;
        if sites.is_empty() && mac.is_empty() {
            return Ok(Some(Vec::new()));
        }
        if !crypto::mac_matches(key, &message(&inner.vault_id, id, &sites), &mac) {
            return Ok(None);
        }
        Ok(Some(serde_json::from_str(&sites).unwrap_or_default()))
    }

    /// The secret's sites, if their MAC checks; none if it doesn't.
    pub(super) fn sites_of(inner: &Inner, key: &[u8; 32], id: &str) -> Result<Vec<String>, VaultError> {
        Ok(Self::sites_checked(inner, key, Self::db(inner)?, id)?.unwrap_or_default())
    }

    /// The sites shown in Settings → Secrets: as kept while the vault is
    /// locked (they aren't secret), and with a warning when their MAC
    /// doesn't check.
    pub(super) fn sites_view(inner: &Inner, db: &Connection, secret: &mut v1::SecretInfo) -> Result<(), VaultError> {
        let Ok(key) = Self::grant_key(inner) else {
            let sites: String = db.query_row("SELECT sites FROM secrets WHERE id = ?1", [&secret.id], |row| row.get(0))?;
            secret.sites = serde_json::from_str(&sites).unwrap_or_default();
            return Ok(());
        };
        match Self::sites_checked(inner, &key, db, &secret.id)? {
            Some(sites) => secret.sites = sites,
            None => secret.warnings.push("Its sites were changed outside Claktory, so it can't be filled anywhere until you set them again.".to_owned()),
        }
        Ok(())
    }

    /// Binds the secret to `sites` (replacing its sites; none unbinds it),
    /// each an exact https origin. Returns its name and the sites as kept.
    pub(crate) fn set_sites(&self, id: &str, sites: &[String]) -> Result<(String, Vec<String>), VaultError> {
        let sites = checked(sites)?;
        let mut inner = self.lock();
        let key = Self::grant_key(&inner)?;
        let vault_id = inner.vault_id.clone();
        let db = Self::db(&inner)?;
        let (id, name) = Self::secret_id(db, id)?;
        if Self::kind_of(db, &id)? == connections::KIND_CONNECTION {
            return Err(VaultError::Invalid(format!("{name} is a connected service: only Claktory uses it, so it's filled nowhere.")));
        }
        let (text, mac) = if sites.is_empty() {
            (String::new(), Vec::new())
        } else {
            let text = serde_json::to_string(&sites).unwrap_or_default();
            let mac = crypto::mac(&key, &message(&vault_id, &id, &text));
            (text, mac)
        };
        db.execute("UPDATE secrets SET sites = ?2, sites_mac = ?3, updated = ?4 WHERE id = ?1", params![id, text, mac, now_ms()])?;
        inner.state = None;
        Ok((name, sites))
    }

    /// The secret named `name` (any case) for one of `agent`'s fills: it
    /// must hold it (a full or fill-only grant whose MAC checks), and the
    /// secret must have sites and not ask each time. Or why not, in words
    /// for the agent; a secret it doesn't hold is said as one the vault
    /// doesn't have.
    pub(crate) fn fillable(&self, agent: &str, name: &str) -> Result<Fillable, String> {
        let inner = self.lock();
        let not_yours = || format!("\"{}\" isn't one of your secrets.", name.trim());
        let Some(db) = &inner.db else { return Err(not_yours()) };
        let (Ok(key), Ok(vmk)) = (Self::grant_key(&inner), Self::open_vmk(&inner)) else {
            return Err("The vault is locked, so nothing can be filled: try again later.".to_owned());
        };
        let read = || -> Result<Result<Fillable, String>, VaultError> {
            let found: Option<(String, String, bool, String)> = db
                .query_row("SELECT id, name, ask, kind FROM secrets WHERE name = ?1", [name.trim()], crate::store::row4)
                .optional()?;
            let Some((id, name, ask, kind)) = found else { return Ok(Err(not_yours())) };
            let Some(Grant { .. }) = Self::grant_of(&inner, &key, &id, agent)? else { return Ok(Err(not_yours())) };
            if kind == connections::KIND_CONNECTION {
                return Ok(Err(not_yours()));
            }
            if ask {
                return Ok(Err(format!("{name} asks the person each time it's used, so it can't be filled: they can turn Ask each time off for it in Settings → Secrets.")));
            }
            let Some(sites) = Self::sites_checked(&inner, &key, db, &id)? else {
                return Ok(Err(format!("{name}'s sites were changed outside Claktory, so it can't be filled until the person sets them again in Settings → Secrets.")));
            };
            if sites.is_empty() {
                return Ok(Err(format!("{name} isn't bound to any site, so it can't be filled: the person adds the sites it's for in Settings → Secrets.")));
            }
            let values = Self::values_of(db, &inner.vault_id, vmk, &id)?.into_iter().filter(|value| !value.file && !value.binary).collect();
            Ok(Ok(Fillable { id, name, sites, values }))
        };
        read().unwrap_or_else(|error| Err(error.to_string()))
    }

    /// What `agent` can fill, one line each: "Shop (USERNAME, PASSWORD)
    /// on https://shop.example". Its secrets with sites, that don't ask
    /// each time; none while the vault is locked.
    pub(crate) fn fillable_lines(&self, agent: &str) -> Vec<String> {
        let inner = self.lock();
        let (Ok(key), Ok(db)) = (Self::grant_key(&inner), Self::db(&inner)) else { return Vec::new() };
        let read = || -> Result<Vec<String>, VaultError> {
            let rows: Vec<(String, String)> = crate::store::query(
                db,
                "SELECT secrets.id, secrets.name FROM grants JOIN secrets ON secrets.id = grants.secret
                 WHERE grants.agent = ?1 AND secrets.ask = 0 AND secrets.sites != '' ORDER BY secrets.name COLLATE NOCASE",
                [agent],
                crate::store::row2,
            )?;
            let mut lines = Vec::new();
            for (id, name) in rows {
                if Self::grant_of(&inner, &key, &id, agent)?.is_none() {
                    continue;
                }
                let Some(sites) = Self::sites_checked(&inner, &key, db, &id)?.filter(|sites| !sites.is_empty()) else { continue };
                let variables: Vec<String> = crate::store::query(
                    db,
                    "SELECT name FROM variables WHERE secret = ?1 AND file = 0 AND binary = 0 ORDER BY position",
                    [&id],
                    |row| row.get(0),
                )?;
                lines.push(format!("{name} ({}) on {}", variables.join(", "), sites.join(", ")));
            }
            Ok(lines)
        };
        read().unwrap_or_else(|error| {
            tracing::error!(%error, agent, "could not read what an agent can fill");
            Vec::new()
        })
    }

    /// Logs a fill: kind `fill`, the agent, the errand as its task, and
    /// where (the origin and the field's kind), never the value.
    pub(crate) fn record_fill(&self, id: &str, agent: &str, errand: &str, detail: &str) -> Result<(), VaultError> {
        let mut inner = self.lock();
        let db = Self::db(&inner)?;
        let name: String = db.query_row("SELECT name FROM secrets WHERE id = ?1", [id], |row| row.get(0))?;
        Self::log(db, id, &name, "fill", agent, errand, detail)?;
        db.execute("UPDATE secrets SET last_used_by = ?2, last_used = ?3 WHERE id = ?1", params![id, agent, now_ms()])?;
        inner.state = None;
        Ok(())
    }

    /// The secret Claktory's browser signed in with at identity provider
    /// `provider` ("google", "apple", "microsoft"), if any: its id.
    pub(crate) fn browser_account(&self, provider: &str) -> Option<String> {
        let inner = self.lock();
        let db = inner.db.as_ref()?;
        db.query_row("SELECT secret FROM browser_accounts WHERE provider = ?1", [provider], |row| row.get(0)).optional().ok().flatten()
    }

    /// Notes that the browser signs in at `provider` with secret `id`; true
    /// when it's the first time (the person is told, with Sign out). False
    /// when it had it already.
    pub(crate) fn set_browser_account(&self, provider: &str, id: &str) -> Result<bool, VaultError> {
        let inner = self.lock();
        let db = Self::db(&inner)?;
        let added = db.execute(
            "INSERT OR IGNORE INTO browser_accounts (provider, secret, since) VALUES (?1, ?2, ?3)",
            params![provider, id, now_ms()],
        )?;
        Ok(added == 1)
    }

    /// Forgets the browser's account at `provider`, or at every one (None):
    /// it was signed out, or the browser's data deleted.
    pub(crate) fn forget_browser_accounts(&self, provider: Option<&str>) -> Result<(), VaultError> {
        let inner = self.lock();
        let db = Self::db(&inner)?;
        match provider {
            Some(provider) => db.execute("DELETE FROM browser_accounts WHERE provider = ?1", [provider])?,
            None => db.execute("DELETE FROM browser_accounts", [])?,
        };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{temp, value, vault};
    use super::*;

    fn shop(vault: &Vault) -> String {
        vault.create(Some("correct horse battery"), true).expect("create").expect("recovery");
        let values = [value("USERNAME", "matt@example.com"), value("PASSWORD", "correct horse battery staple")];
        vault.create_secret("Shop", "", &values, "").expect("secret")
    }

    #[test]
    fn sites_are_exact_https_origins() {
        assert_eq!(
            checked(&["https://Shop.Example/".to_owned(), "https://shop.example".to_owned(), " ".to_owned()]).unwrap(),
            ["https://shop.example"]
        );
        for refused in ["http://shop.example", "https://*.shop.example", "shop.example", "https://shop.example/login", "https://localhost"] {
            assert!(checked(&[refused.to_owned()]).is_err(), "{refused}");
        }
        let many: Vec<String> = (0..=MOST_SITES).map(|n| format!("https://s{n}.example")).collect();
        assert!(checked(&many).is_err());
    }

    #[test]
    fn a_secret_is_filled_only_once_bound_and_held() {
        let dir = temp();
        let vault = vault(&dir);
        let id = shop(&vault);
        vault.grant_fill_only(&id, "errander", "").expect("grant");
        let unbound = vault.fillable("errander", "Shop").unwrap_err();
        assert!(unbound.contains("isn't bound to any site"), "{unbound}");
        assert!(vault.fillable_lines("errander").is_empty());
        let (_, sites) = vault.set_sites(&id, &["https://shop.example".to_owned()]).expect("bound");
        assert_eq!(sites, ["https://shop.example"]);
        let fillable = vault.fillable("errander", "shop").expect("fillable");
        assert_eq!(fillable.sites, ["https://shop.example"]);
        assert_eq!(fillable.values.len(), 2);
        assert_eq!(vault.fillable_lines("errander"), ["Shop (USERNAME, PASSWORD) on https://shop.example"]);
        // Never said apart from a secret the vault doesn't have.
        let nobody = vault.fillable("stranger", "Shop").unwrap_err();
        assert_eq!(nobody, vault.fillable("stranger", "Nothing").unwrap_err().replace("Nothing", "Shop"));
        // Asking each time, it can't be filled: no one could be asked.
        vault.set_ask_each_time(&id, true).expect("ask");
        assert!(vault.fillable("errander", "Shop").unwrap_err().contains("asks the person each time"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_fill_only_grant_never_reaches_a_command_or_an_app() {
        let dir = temp();
        let vault = vault(&dir);
        let id = shop(&vault);
        vault.set_sites(&id, &["https://shop.example".to_owned()]).expect("bound");
        vault.grant_fill_only(&id, "errander", "").expect("grant");
        let refused = vault.resolve("errander", &["Shop".to_owned()]).unwrap_err();
        assert!(refused.contains("only to fill"), "{refused}");
        vault.grant_fill_only(&id, "app:notes", "").expect("grant");
        assert!(vault.value_for(&id, "app:notes").expect("read").is_none());
        let label = vault.granted("errander").remove(&id).expect("listed");
        assert!(label.starts_with("Shop (sets USERNAME, PASSWORD; fill only"), "{label}");
        let state = vault.state(|_, _| false);
        assert!(state.secrets[0].grants.iter().all(|grant| grant.fill_only));
        assert_eq!(state.secrets[0].sites, ["https://shop.example"]);
        // A full grant replaces it, and still fills.
        vault.grant(&id, "errander", "").expect("grant");
        assert!(vault.resolve("errander", &["Shop".to_owned()]).is_ok());
        assert!(vault.fillable("errander", "Shop").is_ok());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn sites_and_fill_only_grants_edited_outside_claktory_count_for_nothing() {
        let dir = temp();
        let vault = vault(&dir);
        let id = shop(&vault);
        vault.set_sites(&id, &["https://shop.example".to_owned()]).expect("bound");
        vault.grant_fill_only(&id, "errander", "").expect("grant");
        {
            let inner = vault.lock();
            let db = inner.db.as_ref().expect("db");
            // Rebound to another site, and the grant made a full one.
            db.execute("UPDATE secrets SET sites = '[\"https://evil.example\"]' WHERE id = ?1", [&id]).expect("tamper");
            db.execute("UPDATE grants SET fill_only = 0 WHERE agent = 'errander'", []).expect("tamper");
        }
        vault.lock().state = None;
        assert!(vault.resolve("errander", &["Shop".to_owned()]).is_err(), "no command gets it");
        let refused = vault.fillable("errander", "Shop").unwrap_err();
        assert!(refused.contains("isn't one of your secrets"), "the grant counts for nothing: {refused}");
        let state = vault.state(|_, _| false);
        assert!(state.secrets[0].sites.is_empty());
        assert!(state.secrets[0].warnings.iter().any(|warning| warning.contains("sites were changed outside Claktory")), "{:?}", state.secrets[0].warnings);
        // Set again by the person, it counts.
        vault.set_sites(&id, &["https://shop.example".to_owned()]).expect("bound");
        vault.grant_fill_only(&id, "errander", "").expect("grant");
        assert_eq!(vault.fillable("errander", "Shop").expect("fillable").sites, ["https://shop.example"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn fills_are_logged_and_the_browser_keeps_one_account_per_provider() {
        let dir = temp();
        let vault = vault(&dir);
        let id = shop(&vault);
        vault.record_fill(&id, "errander", "e1", "filled into a password field on https://shop.example").expect("logged");
        let uses = vault.uses(&id, 10).expect("uses");
        assert_eq!((uses[0].kind.as_str(), uses[0].agent_id.as_str(), uses[0].task_id.as_str()), ("fill", "errander", "e1"));
        assert!(!uses[0].detail.contains("correct horse"));
        assert_eq!(vault.browser_account("Google"), None);
        assert!(vault.set_browser_account("Google", &id).expect("kept"), "the first time");
        assert!(!vault.set_browser_account("Google", "other").expect("kept"), "not a second account");
        assert_eq!(vault.browser_account("Google").as_deref(), Some(id.as_str()));
        vault.forget_browser_accounts(None).expect("forgot");
        assert_eq!(vault.browser_account("Google"), None);
        let _ = std::fs::remove_dir_all(dir);
    }
}
