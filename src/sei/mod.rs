mod client;
mod connect;
mod discovery;
mod errors;
mod html;
mod http;
mod json;
mod orgs;
mod types;
mod wire;

pub use client::{Client, Credentials, PROCESS_PAGE_SIZE};
pub use connect::{Connection, Target, connect};
pub use discovery::discover;
pub use errors::{ApiError, ErrorKind, SeiError};
pub use http::{Options, REQUEST_TIMEOUT};
pub use orgs::{list_orgs, resolve_org};
pub use types::{
    Activity, Document, Org, Page, Process, ProcessQuery, RawPage, Session, Unit, User, Version,
};
