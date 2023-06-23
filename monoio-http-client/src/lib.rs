#![feature(type_alias_impl_trait)]
#![feature(impl_trait_in_assoc_type)]

mod client;
mod error;
mod request;
mod response;

pub use client::{connector::Connector, Builder, Client, ClientConfig};
pub use error::{Error, Result};
pub use request::ClientRequest;
pub use response::ClientResponse;
