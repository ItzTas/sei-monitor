use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "accounts")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub sei_base_url: String,
    pub username: String,
    pub org_id: i64,
    pub created_at: DateTimeUtc,
    pub baseline_at: Option<DateTimeUtc>,
    pub paused_reason: String,
    pub org_acronym: String,
    pub sei_version: String,
    pub wssei_version: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
