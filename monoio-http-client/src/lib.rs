#![allow(stable_features)]
#![feature(generic_associated_types)]
#![feature(type_alias_impl_trait)]

mod client;
mod error;
mod request;
mod response;

pub use client::{connector::Connector, Client, ClientConfig};
pub use error::{Error, Result};
pub use request::ClientRequest;
pub use response::ClientResponse;
