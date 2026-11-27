//! Where a secret may be filled in Claktory's browser: its sites, exact
//! https origins as `browser::origin` spells them, never a pattern or a
//! domain ending, kept with a MAC from the grants' key so editing vault.db
//! can't bind a secret to another site; the values a fill gets, for an
//! agent that holds the secret (a full grant or a fill-only one); every
//! fill logged in `uses`; and the one account Claktory's browser signed in
//! with at each identity provider.

use rusqlite::{Connection, params};

use super::{Inner, Value, Vault, VaultError, aad, connections, crypto, now_ms};
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
        if !sites.is_empty() {
            db.execute("UPDATE variables SET hint = '' WHERE secret = ?1 AND name != ?2", params![id, super::logins::USERNAME])?;
        }
        inner.state = None;
        Ok((name, sites))
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
        assert!(vault.login_for("errander", "https://shop.example").expect("read").is_none(), "bound to no site");
        let (_, sites) = vault.set_sites(&id, &["https://shop.example".to_owned()]).expect("bound");
        assert_eq!(sites, ["https://shop.example"]);
        let login = vault.login_for("errander", "https://shop.example").expect("read").expect("its login");
        assert_eq!(login.sites, ["https://shop.example"]);
        assert_eq!(login.values.len(), 2);
        assert!(vault.login_for("errander", "https://other.example").expect("read").is_none(), "only on its sites");
        assert!(vault.login_for("stranger", "https://shop.example").expect("read").is_none(), "only to whom holds it");
        // Asking each time, it can't be filled: no one could be asked.
        vault.set_ask_each_time(&id, true).expect("ask");
        assert!(vault.login_for("errander", "https://shop.example").expect("read").is_none());
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
        assert!(refused.contains("no command can have it"), "{refused}");
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
        assert!(vault.login_for("errander", "https://shop.example").expect("read").is_some());
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
        assert!(vault.login_for("errander", "https://evil.example").expect("read").is_none(), "the grant counts for nothing");
        let state = vault.state(|_, _| false);
        assert!(state.secrets[0].sites.is_empty());
        assert!(state.secrets[0].warnings.iter().any(|warning| warning.contains("sites were changed outside Claktory")), "{:?}", state.secrets[0].warnings);
        // Set again by the person, it counts.
        vault.set_sites(&id, &["https://shop.example".to_owned()]).expect("bound");
        vault.grant_fill_only(&id, "errander", "").expect("grant");
        assert_eq!(vault.login_for("errander", "https://shop.example").expect("read").expect("its login").sites, ["https://shop.example"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn fills_are_logged_without_the_value() {
        let dir = temp();
        let vault = vault(&dir);
        let id = shop(&vault);
        vault.record_fill(&id, "errander", "e1", "filled into a password field on https://shop.example").expect("logged");
        let uses = vault.uses(&id, 10).expect("uses");
        assert_eq!((uses[0].kind.as_str(), uses[0].agent_id.as_str(), uses[0].task_id.as_str()), ("fill", "errander", "e1"));
        assert!(!uses[0].detail.contains("correct horse"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
