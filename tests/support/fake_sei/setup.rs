use chrono::{DateTime, Utc};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer};
use serde_json::Value;

use super::{FakeActivity, FakeDocument, FakeProcess, FakeSei, FakeUser};

#[derive(Deserialize)]
#[serde(
    tag = "call",
    content = "args",
    rename_all = "snake_case",
    deny_unknown_fields
)]
enum SetupCall {
    SetProcesses(SetProcesses),
    AddActivities(AddActivities),
    AddDocuments(AddDocuments),
    FailNext(FailNext),
    SetModuleInactive(SetModuleInactive),
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct SetProcesses {
    unit_id: i64,
    processes: Vec<RecordedProcess>,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct AddActivities {
    procedure_id: i64,
    activities: Vec<RecordedActivity>,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct AddDocuments {
    procedure_id: i64,
    documents: Vec<RecordedDocument>,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct FailNext {
    route: String,
    status: u16,
    times: usize,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct SetModuleInactive {
    inactive: bool,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RecordedUser {
    id: i64,
    acronym: String,
    name: String,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RecordedProcess {
    id: i64,
    number: String,
    #[serde(rename = "type")]
    process_type: String,
    description: String,
    assignee: Option<RecordedUser>,
    return_date: String,
    return_overdue: bool,
    generated: bool,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RecordedActivity {
    id: i64,
    user: String,
    unit: String,
    text: String,
    #[serde(deserialize_with = "rfc3339")]
    at: DateTime<Utc>,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RecordedDocument {
    id: i64,
    protocol: String,
    #[serde(rename = "type")]
    document_type: String,
    number: String,
    label: String,
}

fn rfc3339<'de, D: Deserializer<'de>>(deserializer: D) -> Result<DateTime<Utc>, D::Error> {
    let text = String::deserialize(deserializer)?;
    DateTime::parse_from_rfc3339(&text)
        .map(|at| at.with_timezone(&Utc))
        .map_err(|error| D::Error::custom(format!("activity time {text:?}: {error}")))
}

impl FakeSei {
    pub fn apply(&self, calls: &[Value]) -> Result<(), serde_json::Error> {
        let calls = calls
            .iter()
            .map(SetupCall::deserialize)
            .collect::<Result<Vec<_>, _>>()?;
        calls.into_iter().for_each(|call| self.apply_call(call));
        Ok(())
    }

    fn apply_call(&self, call: SetupCall) {
        match call {
            SetupCall::SetProcesses(args) => {
                let processes = args.processes.into_iter().map(FakeProcess::from).collect();
                self.set_processes(args.unit_id, processes);
            }
            SetupCall::AddActivities(args) => {
                let activities = args
                    .activities
                    .into_iter()
                    .map(FakeActivity::from)
                    .collect();
                self.add_activities(args.procedure_id, activities);
            }
            SetupCall::AddDocuments(args) => {
                let documents = args.documents.into_iter().map(FakeDocument::from).collect();
                self.add_documents(args.procedure_id, documents);
            }
            SetupCall::FailNext(args) => self.fail_next(&args.route, args.status, args.times),
            SetupCall::SetModuleInactive(args) => self.set_module_inactive(args.inactive),
        }
    }
}

impl From<RecordedProcess> for FakeProcess {
    fn from(process: RecordedProcess) -> FakeProcess {
        FakeProcess {
            id: process.id,
            number: process.number,
            process_type: process.process_type,
            description: process.description,
            assignee: process.assignee.map(|user| FakeUser {
                id: user.id,
                acronym: user.acronym,
                name: user.name,
            }),
            return_date: process.return_date,
            return_overdue: process.return_overdue,
            generated: process.generated,
            generating_unit: None,
        }
    }
}

impl From<RecordedActivity> for FakeActivity {
    fn from(activity: RecordedActivity) -> FakeActivity {
        FakeActivity {
            id: activity.id,
            user: activity.user,
            unit: activity.unit,
            text: activity.text,
            at: activity.at,
        }
    }
}

impl From<RecordedDocument> for FakeDocument {
    fn from(document: RecordedDocument) -> FakeDocument {
        FakeDocument {
            id: document.id,
            protocol: document.protocol,
            document_type: document.document_type,
            number: document.number,
            label: document.label,
        }
    }
}
