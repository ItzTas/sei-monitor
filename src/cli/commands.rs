use std::io::Write;
use std::iter;

use chrono_tz::UTC;

use super::{open, options, units_error};
use crate::config::{SeiConfig, Source, load_sei_url};
use crate::sei::{Org, list_orgs};

const TABLE_PADDING: usize = 2;

pub async fn orgs(source: &Source, out: &mut dyn Write) -> anyhow::Result<()> {
    let sei_url = load_sei_url(source)?;
    let mut orgs = list_orgs(&sei_url, &options(UTC)).await?;
    orgs.sort_by(|a, b| a.acronym.cmp(&b.acronym));
    let rows: Vec<(&str, String)> = iter::once(("ACRONYM", "ID".to_owned()))
        .chain(
            orgs.iter()
                .map(|org| (org.acronym.as_str(), org.id.to_string())),
        )
        .collect();
    let width = rows
        .iter()
        .map(|(acronym, _)| acronym.chars().count() + TABLE_PADDING)
        .max()
        .unwrap_or(TABLE_PADDING);
    for (acronym, id) in rows {
        let padding = " ".repeat(width - acronym.chars().count());
        writeln!(out, "{acronym}{padding}{id}")?;
    }
    Ok(())
}

pub async fn test_login(source: &Source, out: &mut dyn Write) -> anyhow::Result<()> {
    let config = SeiConfig::load(source)?;
    let connection = open(&config).await?;
    let session = connection.client.login().await?;
    let version = connection.client.version().await?;
    let watched = session
        .select_units(&config.units, config.all_units)
        .map_err(units_error)?;

    writeln!(out, "API:       {}", connection.api_url)?;
    writeln!(out, "Agency:    {}", describe_org(&connection.org))?;
    writeln!(
        out,
        "Login:     ok, as {} ({})",
        session.user_acronym, session.user_name
    )?;
    writeln!(
        out,
        "Versions:  SEI {}, mod-wssei {}",
        version.sei, version.module
    )?;
    writeln!(out, "Your units:")?;
    for unit in &session.units {
        let current = if unit.id == session.current_unit_id {
            " (current)"
        } else {
            ""
        };
        writeln!(out, "  {}{current}", unit.acronym)?;
    }
    let watched: Vec<&str> = watched.iter().map(|unit| unit.acronym.as_str()).collect();
    writeln!(out, "Watched:   {}", watched.join(", "))?;
    Ok(())
}

fn describe_org(org: &Org) -> String {
    if org.acronym.is_empty() {
        return format!("id {}", org.id);
    }
    format!("{} (id {})", org.acronym, org.id)
}
