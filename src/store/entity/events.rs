use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "events")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub account_id: i64,
    pub kind: String,
    pub procedure_id: i64,
    #[sea_orm(column_name = "ref")]
    pub reference: String,
    pub occurred_at: DateTimeUtc,
    pub detected_at: DateTimeUtc,
    pub payload: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
