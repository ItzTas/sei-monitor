use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "documents")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub account_id: i64,
    #[sea_orm(primary_key, auto_increment = false)]
    pub document_id: i64,
    pub procedure_id: i64,
    pub formatted_protocol: String,
    pub document_type: String,
    pub name: String,
    pub first_seen_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
