use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "processes")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub account_id: i64,
    #[sea_orm(primary_key, auto_increment = false)]
    pub procedure_id: i64,
    pub number: String,
    pub process_type: String,
    pub description: String,
    pub unit_id: i64,
    pub unit_acronym: String,
    pub assigned_user: String,
    pub assigned_user_id: i64,
    pub return_date: String,
    pub return_overdue: bool,
    pub last_activity_id: i64,
    pub document_total: i64,
    pub first_seen_at: DateTimeUtc,
    pub last_seen_at: DateTimeUtc,
    pub left_at: Option<DateTimeUtc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
