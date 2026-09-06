use async_trait::async_trait;

use super::{Credential, CredentialError, CredentialSource};
use crate::config::CredentialHeader;

/// A credential fixed for the life of the process, or none at all.
#[derive(Debug)]
pub struct FixedCredential {
    credential: Option<Credential>,
    /// Set when the value was read from this environment variable.
    env_name: Option<String>,
}

impl FixedCredential {
    pub fn none() -> Self {
        Self {
            credential: None,
            env_name: None,
        }
    }

    pub fn secret(header: CredentialHeader, value: String) -> Result<Self, CredentialError> {
        Ok(Self {
            credential: Some(Credential::new(header, value)?),
            env_name: None,
        })
    }

    /// Reads `name` from the process environment once, at construction.
    pub fn from_env(header: CredentialHeader, name: &str) -> Result<Self, CredentialError> {
        let value =
            std::env::var(name).map_err(|_| CredentialError::MissingEnv(name.to_owned()))?;
        Ok(Self {
            credential: Some(Credential::new(header, value)?),
            env_name: Some(name.to_owned()),
        })
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
            (Some(name), _) => format!("env ${name}"),
            (None, Some(_)) => "static".to_owned(),
            (None, None) => "none".to_owned(),
        }
    }
}
