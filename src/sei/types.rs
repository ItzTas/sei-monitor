use chrono::{DateTime, NaiveDate, Utc};
use serde_json::Value;

use crate::gotext::{equal_fold, trim};

#[derive(Clone, Debug, PartialEq)]
pub struct Org {
    pub id: i64,
    pub acronym: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Unit {
    pub id: i64,
    pub acronym: String,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    pub user_id: i64,
    pub user_acronym: String,
    pub user_name: String,
    pub current_unit_id: i64,
    pub units: Vec<Unit>,
}

impl Session {
    pub fn unit_by_id(&self, id: i64) -> Option<&Unit> {
        self.units.iter().find(|unit| unit.id == id)
    }

    pub fn unit_by_acronym(&self, acronym: &str) -> Option<&Unit> {
        let wanted = trim(acronym);
        self.units
            .iter()
            .find(|unit| equal_fold(&unit.acronym, wanted))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Version {
    pub sei: String,
    pub module: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct User {
    pub id: i64,
    pub acronym: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Process {
    pub procedure_id: i64,
    pub number: String,
    pub process_type: String,
    pub description: String,
    pub assignee: Option<User>,
    pub return_date: Option<NaiveDate>,
    pub return_overdue: bool,
    pub origin: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Activity {
    pub id: i64,
    pub procedure_id: i64,
    pub user: String,
    pub unit: String,
    pub text: String,
    pub at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Document {
    pub id: i64,
    pub procedure_id: i64,
    pub protocol: String,
    pub document_type: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub total: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProcessQuery {
    pub page: usize,
    pub limit: usize,
    pub kind: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RawPage {
    pub data: Option<Value>,
    pub total: Option<Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(id: i64, acronym: &str) -> Unit {
        Unit {
            id,
            acronym: acronym.to_owned(),
            description: String::new(),
        }
    }

    #[test]
    fn units_are_found_by_id_and_by_acronym_in_any_case() {
        let session = Session {
            user_id: 101,
            user_acronym: "fulano.tal".to_owned(),
            user_name: "Fulano de Tal".to_owned(),
            current_unit_id: 1001,
            units: vec![unit(1001, "SEDEMO/NUPROC"), unit(1002, "SEDEMO/GAB")],
        };

        assert_eq!(session.unit_by_id(1002), Some(&session.units[1]));
        assert_eq!(session.unit_by_id(7), None);
        assert_eq!(
            session.unit_by_acronym(" sedemo/nuproc\t"),
            Some(&session.units[0])
        );
        assert_eq!(session.unit_by_acronym("SEDEMO"), None);
    }
}
