use async_trait::async_trait;

use super::{Credential, CredentialError, CredentialSource};
use crate::config::CredentialHeader;

/// A credential fixed for the life of the process, or none at all.
#[derive(Debug)]
pub struct FixedCredential {
    credential: Option<Credential>,
    origin: &'static str,
    env_name: Option<String>,
}

impl FixedCredential {
    pub fn none() -> Self {
        Self {
            credential: None,
            origin: "none",
            env_name: None,
        }
    }

    pub fn secret(header: CredentialHeader, value: String) -> Result<Self, CredentialError> {
        Ok(Self {
            credential: Some(Credential::new(header, value)?),
            origin: "static",
            env_name: None,
        })
    }

    /// Reads `name` from the process environment once, at construction.
    pub fn from_env(header: CredentialHeader, name: &str) -> Result<Self, CredentialError> {
        let value =
            std::env::var(name).map_err(|_| CredentialError::MissingEnv(name.to_owned()))?;
        let mut fixed = Self::secret(header, value)?;
        fixed.origin = "env";
        fixed.env_name = Some(name.to_owned());
        Ok(fixed)
    }
}

#[async_trait]
impl CredentialSource for FixedCredential {
    async fn credential(&self) -> Result<Option<Credential>, CredentialError> {
        Ok(self.credential.clone())
    }

    async fn invalidate(&self) {}

    fn describe(&self) -> String {
        match (&self.env_name, &self.credential) {
            (Some(name), Some(c)) => format!("env ${name} ({})", c.masked()),
            (None, Some(c)) => format!("static ({})", c.masked()),
            _ => self.origin.to_owned(),
        }
    }
}
