mod connection;
mod error;
mod go_away;
mod peer;
mod ping_pong;
mod settings;
mod streams;

use bytes::Buf;

pub use self::error::{Error, Initiator};
pub(crate) use self::{
    connection::{Config, Connection},
    peer::{Dyn as DynPeer, Peer},
    ping_pong::UserPings,
    streams::{DynStreams, OpaqueStreamRef, Open, PollReset, Prioritized, StreamRef, Streams},
};
use self::{go_away::GoAway, ping_pong::PingPong, settings::Settings};
use crate::h2::{
    codec::Codec,
    frame::{self, Frame},
};

pub type PingPayload = [u8; 8];

pub type WindowSize = u32;

// Constants
pub const MAX_WINDOW_SIZE: WindowSize = (1 << 31) - 1;
pub const DEFAULT_RESET_STREAM_MAX: usize = 10;
pub const DEFAULT_RESET_STREAM_SECS: u64 = 30;
pub const DEFAULT_MAX_SEND_BUFFER_SIZE: usize = 1024 * 400;
