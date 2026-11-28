use std::sync::Arc;

use super::client::{Client, Credentials};
use super::discovery::discover;
use super::errors::SeiError;
use super::http::Options;
use super::orgs::resolve_org;
use super::types::{Org, Session, Unit};
use crate::password::PasswordSource;

#[derive(Clone)]
pub struct Target {
    pub sei_url: String,
    pub api_url: String,
    pub org: String,
    pub username: String,
    pub password: Arc<dyn PasswordSource>,
}

#[derive(Clone)]
pub struct Connection {
    pub client: Client,
    pub org: Org,
    pub api_url: String,
}

pub async fn connect(target: &Target, options: &Options) -> Result<Connection, SeiError> {
    let (api_url, org) = tokio::try_join!(
        api_url(target, options),
        resolve_org(&target.sei_url, &target.org, options)
    )?;
    let credentials = Credentials {
        username: target.username.clone(),
        password: Arc::clone(&target.password),
        org_id: org.id,
    };
    Ok(Connection {
        client: Client::new(&api_url, credentials, options),
        org,
        api_url,
    })
}

async fn api_url(target: &Target, options: &Options) -> Result<String, SeiError> {
    if !target.api_url.is_empty() {
        return Ok(target.api_url.clone());
    }
    discover(&target.sei_url, options).await
}

impl Session {
    pub fn select_units(&self, acronyms: &[String], all: bool) -> Result<Vec<Unit>, SeiError> {
        if all {
            return Ok(self.units.clone());
        }
        if acronyms.is_empty() {
            return Ok(vec![self.current_unit()]);
        }
        acronyms
            .iter()
            .map(|acronym| {
                self.unit_by_acronym(acronym)
                    .cloned()
                    .ok_or_else(|| self.unknown_unit(acronym))
            })
            .collect()
    }

    fn current_unit(&self) -> Unit {
        self.unit_by_id(self.current_unit_id)
            .cloned()
            .unwrap_or_else(|| Unit {
                id: self.current_unit_id,
                acronym: String::new(),
                description: String::new(),
            })
    }

    fn unknown_unit(&self, acronym: &str) -> SeiError {
        SeiError::UnknownUnit {
            acronym: acronym.to_owned(),
            yours: self.units.iter().map(|unit| unit.acronym.clone()).collect(),
        }
    }
}
