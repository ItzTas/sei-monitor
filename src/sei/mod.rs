#[cfg_attr(not(test), expect(dead_code))]
mod errors;
#[cfg_attr(not(test), expect(dead_code))]
mod html;
#[cfg_attr(not(test), expect(dead_code))]
mod json;
mod types;
#[cfg_attr(not(test), expect(dead_code))]
mod wire;

pub use errors::{ApiError, ErrorKind, SeiError};
pub use types::{
    Activity, Document, Org, Page, Process, ProcessQuery, RawPage, Session, Unit, User, Version,
};
