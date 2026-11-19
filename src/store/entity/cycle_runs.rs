use sea_orm::entity::prelude::*;

use crate::store::Trigger;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "cycle_runs")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub account_id: i64,
    pub trigger: Trigger,
    pub started_at: DateTimeUtc,
    pub finished_at: Option<DateTimeUtc>,
    pub error: String,
    pub processes_seen: i64,
    pub events_created: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
