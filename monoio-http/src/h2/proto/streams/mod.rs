mod buffer;
mod counts;
mod flow_control;
mod prioritize;
mod recv;
mod send;
mod state;
mod store;
mod stream;
#[allow(clippy::module_inception)]
mod streams;

use std::time::Duration;

use bytes::Bytes;

use self::{
    buffer::Buffer, counts::Counts, flow_control::FlowControl, prioritize::Prioritize, recv::Recv,
    send::Send, state::State, store::Store, stream::Stream,
};
pub(crate) use self::{
    prioritize::Prioritized,
    recv::Open,
    send::PollReset,
    streams::{DynStreams, OpaqueStreamRef, StreamRef, Streams},
};
use crate::h2::{
    frame::{StreamId, StreamIdOverflow},
    proto::*,
};

#[derive(Debug)]
pub struct Config {
    /// Initial window size of locally initiated streams
    pub local_init_window_sz: WindowSize,

    /// Initial maximum number of locally initiated streams.
    /// After receiving a Settings frame from the remote peer,
    /// the connection will overwrite this value with the
    /// MAX_CONCURRENT_STREAMS specified in the frame.
    pub initial_max_send_streams: usize,

    /// Max amount of DATA bytes to buffer per stream.
    pub local_max_buffer_size: usize,

    /// The stream ID to start the next local stream with
    pub local_next_stream_id: StreamId,

    /// If the local peer is willing to receive push promises
    pub local_push_enabled: bool,

    /// If extended connect protocol is enabled.
    pub extended_connect_protocol_enabled: bool,

    /// How long a locally reset stream should ignore frames
    pub local_reset_duration: Duration,

    /// Maximum number of locally reset streams to keep at a time
    pub local_reset_max: usize,

    /// Initial window size of remote initiated streams
    pub remote_init_window_sz: WindowSize,

    /// Maximum number of remote initiated streams
    pub remote_max_initiated: Option<usize>,
}
