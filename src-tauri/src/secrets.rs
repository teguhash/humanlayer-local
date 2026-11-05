//! Secrets in the macOS Keychain (generic passwords under the app's
//! identifier). Values are cached in memory after the first read so the
//! Keychain is asked once per app run. Never log a value.

use std::collections::HashMap;
use std::sync::Mutex;

const SERVICE: &str = "dev.teguh.humanlayerlocal";
pub const LINEAR_API_KEY: &str = "linear-api-key";
pub const GITHUB_TOKEN: &str = "github-token";

pub struct Secrets {
    cache: Mutex<HashMap<&'static str, Option<String>>>,
}

impl Default for Secrets {
    fn default() -> Self {
        Self::new()
    }
}

impl Secrets {
    pub fn new() -> Self {
        Self {
            cache: Mutex::new(HashMap::new()),
        }
    }

    fn entry(name: &str) -> Result<keyring::Entry, String> {
        keyring::Entry::new(SERVICE, name).map_err(|e| format!("Keychain: {e}"))
    }

    pub fn get(&self, name: &'static str) -> Result<Option<String>, String> {
        let mut cache = self.cache.lock().unwrap();
        if let Some(v) = cache.get(name) {
            return Ok(v.clone());
        }
        let v = match Self::entry(name)?.get_password() {
            Ok(k) => Some(k),
            Err(keyring::Error::NoEntry) => None,
            Err(e) => return Err(format!("Could not read {name} from the Keychain: {e}")),
        };
        cache.insert(name, v.clone());
        Ok(v)
    }

    pub fn set(&self, name: &'static str, value: &str) -> Result<(), String> {
        Self::entry(name)?
            .set_password(value)
            .map_err(|e| format!("Could not save {name} to the Keychain: {e}"))?;
        self.cache
            .lock()
            .unwrap()
            .insert(name, Some(value.to_string()));
        Ok(())
    }

    pub fn clear(&self, name: &'static str) -> Result<(), String> {
        match Self::entry(name)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => {}
            Err(e) => return Err(format!("Could not remove {name}: {e}")),
        }
        self.cache.lock().unwrap().insert(name, None);
        Ok(())
    }

    pub fn linear_key(&self) -> Result<Option<String>, String> {
        self.get(LINEAR_API_KEY)
    }

    pub fn set_linear_key(&self, key: &str) -> Result<(), String> {
        self.set(LINEAR_API_KEY, key)
    }

    pub fn clear_linear_key(&self) -> Result<(), String> {
        self.clear(LINEAR_API_KEY)
    }

    /// Env vars that carry saved credentials into gh, the linear CLI and
    /// agent sessions.
    pub fn env_vars(&self) -> Vec<(&'static str, String)> {
        let mut v = vec![];
        for (name, var) in [
            (LINEAR_API_KEY, "LINEAR_API_KEY"),
            (GITHUB_TOKEN, "GH_TOKEN"),
        ] {
            match self.get(name) {
                Ok(Some(k)) => v.push((var, k)),
                Ok(None) => {}
                Err(e) => log::warn!("{e}"),
            }
        }
        v
    }
}
