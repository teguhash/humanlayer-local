//! Secrets in the macOS Keychain (generic passwords under the app's
//! identifier). Values are cached in memory after the first read so the
//! Keychain is asked once per app run. Never log a value.

use std::sync::Mutex;

const SERVICE: &str = "dev.teguh.humanlayerlocal";
pub const LINEAR_API_KEY: &str = "linear-api-key";

pub struct Secrets {
    linear: Mutex<Option<Option<String>>>,
}

impl Secrets {
    pub fn new() -> Self {
        Self {
            linear: Mutex::new(None),
        }
    }

    fn entry() -> Result<keyring::Entry, String> {
        keyring::Entry::new(SERVICE, LINEAR_API_KEY).map_err(|e| format!("Keychain: {e}"))
    }

    /// The Linear API key, if one is saved.
    pub fn linear_key(&self) -> Result<Option<String>, String> {
        let mut cache = self.linear.lock().unwrap();
        if let Some(v) = cache.as_ref() {
            return Ok(v.clone());
        }
        let v = match Self::entry()?.get_password() {
            Ok(k) => Some(k),
            Err(keyring::Error::NoEntry) => None,
            Err(e) => {
                return Err(format!(
                    "Could not read the Linear key from the Keychain: {e}"
                ));
            }
        };
        *cache = Some(v.clone());
        Ok(v)
    }

    pub fn set_linear_key(&self, key: &str) -> Result<(), String> {
        Self::entry()?
            .set_password(key)
            .map_err(|e| format!("Could not save the Linear key to the Keychain: {e}"))?;
        *self.linear.lock().unwrap() = Some(Some(key.to_string()));
        Ok(())
    }

    pub fn clear_linear_key(&self) -> Result<(), String> {
        match Self::entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => {}
            Err(e) => return Err(format!("Could not remove the Linear key: {e}")),
        }
        *self.linear.lock().unwrap() = Some(None);
        Ok(())
    }
}
