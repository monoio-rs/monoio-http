use std::{fmt::Display, future::poll_fn, hash::Hash};

use bytes::Bytes;
use local_sync::{
    mpsc::{self, SendError},
    oneshot,
};
use monoio::io::{sink::SinkExt, stream::Stream, AsyncReadRent, AsyncWriteRent, Split};
use monoio_http::{
    common::{
        body::{Body, StreamHint},
        error::HttpError,
        request::Request,
        response::Response,
    },
    h1::{codec::ClientCodec, payload::FramedPayloadRecvr},
    h2::{RecvStream, SendStream},
};

use super::pool::PooledConnection;

pub struct Transaction<K, B>
where
    K: Hash + Eq + Display,
{
    pub req: Request<B>,
    pub resp_tx: oneshot::Sender<crate::Result<Response<B>>>,
    pub conn: PooledConnection<K, B>,
}

impl<K, B> Transaction<K, B>
where
    K: Hash + Eq + Display,
{
    pub fn parts(
        self,
    ) -> (
        Request<B>,
        oneshot::Sender<crate::Result<Response<B>>>,
        PooledConnection<K, B>,
    ) {
        (self.req, self.resp_tx, self.conn)
    }
}
pub struct SingleRecvr<K, B>
where
    K: Hash + Eq + Display,
{
    pub req_rx: mpsc::unbounded::Rx<Transaction<K, B>>,
}

pub struct SingleSender<K, B>
where
    K: Hash + Eq + Display,
{
    req_tx: mpsc::unbounded::Tx<Transaction<K, B>>,
}

impl<K, B> SingleSender<K, B>
where
    K: Hash + Eq + Display,
{
    pub fn into_multi_sender(self) -> MultiSender<K, B> {
        MultiSender {
            req_tx: self.req_tx,
        }
    }

    // Ideally we should never clone the Single Sender,
    // but we need a temp sender to handle HTTP1
    pub fn temp_sender_clone(&self) -> Self {
        Self {
            req_tx: self.req_tx.clone(),
        }
    }

    pub fn send(&self, item: Transaction<K, B>) -> Result<(), SendError> {
        self.req_tx.send(item)
    }
}

pub struct MultiSender<K, B>
where
    K: Hash + Eq + Display,
{
    pub req_tx: mpsc::unbounded::Tx<Transaction<K, B>>,
}

impl<K, B> MultiSender<K, B>
where
    K: Hash + Eq + Display,
{
    pub fn send(&self, item: Transaction<K, B>) -> Result<(), SendError> {
        self.req_tx.send(item)
    }
}

impl<K, B> Clone for MultiSender<K, B>
where
    K: Hash + Eq + Display,
{
    fn clone(&self) -> Self {
        Self {
            req_tx: self.req_tx.clone(),
        }
    }
}
pub struct Http1ConnManager<IO: AsyncWriteRent, K, B>
where
    K: Hash + Eq + Display,
{
    pub req_rx: SingleRecvr<K, B>,
    pub handle: Option<ClientCodec<IO>>,
}

const CONN_CLOSE: &[u8] = b"close";

impl<IO, K, B> Http1ConnManager<IO, K, B>
where
    IO: AsyncReadRent + AsyncWriteRent + Split,
    K: Hash + Eq + Display,
    B: Body<Data = Bytes, Error = HttpError> + From<FramedPayloadRecvr>,
{
    pub async fn drive(&mut self) {
        let mut codec = self.handle.take().unwrap();

        loop {
            if let Some(t) = self.req_rx.req_rx.recv().await {
                let (request, resp_tx, mut connection) = t.parts();
                let (parts, body) = request.into_parts();

                #[cfg(feature = "logging")]
                tracing::debug!("Response recv on h1 conn {:?}", parts);

                match codec.send_and_flush(Request::from_parts(parts, body)).await {
                    Ok(_) => match codec.next().await {
                        Some(Ok(resp)) => {
                            let (data_tx, data_rx) = local_sync::mpsc::unbounded::channel();

                            let header_value = resp.headers().get(http::header::CONNECTION);
                            let reuse_conn = match header_value {
                                Some(v) => !v.as_bytes().eq_ignore_ascii_case(CONN_CLOSE),
                                None => resp.version() != http::Version::HTTP_10,
                            };
                            connection.set_reuseable(reuse_conn);

                            let (parts, body_builder) = resp.into_parts();
                            let mut framed_payload = body_builder.with_io(codec);
                            let framed_payload_rcvr = FramedPayloadRecvr {
                                data_rx,
                                hint: framed_payload.stream_hint(),
                            };

                            #[cfg(feature = "logging")]
                            tracing::debug!("Sending reply back from conn manager {:?}", parts);

                            let resp = Response::from_parts(parts, B::from(framed_payload_rcvr));
                            let _ = resp_tx.send(Ok(resp));

                            while let Some(r) = framed_payload.next_data().await {
                                let _ = data_tx.send(Some(r));
                            }

                            // At this point we have streamed the payload and the codec can be
                            // reused. Drop the connection, which will
                            // add it back to the pool.

                            drop(connection);
                            codec = framed_payload.get_source();
                        }
                        Some(Err(e)) => {
                            #[cfg(feature = "logging")]
                            tracing::error!("decode upstream response error {:?}", e);
                            connection.set_reuseable(false);
                            let _ = resp_tx.send(Err(crate::Error::H1Decode(e)));
                            break;
                        }
                        None => {
                            #[cfg(feature = "logging")]
                            tracing::error!("upstream return eof");
                            connection.set_reuseable(false);
                            let _ = resp_tx.send(Err(crate::Error::Io(std::io::Error::new(
                                std::io::ErrorKind::UnexpectedEof,
                                "unexpected eof when read response",
                            ))));
                            break;
                        }
                    },
                    Err(e) => {
                        #[cfg(feature = "logging")]
                        tracing::error!("send upstream request error {:?}", e);
                        connection.set_reuseable(false);
                        let _ = resp_tx.send(Err(e.into()));
                    }
                }
            } else {
                break;
            }
        }
    }
}

pub struct StreamTask<B: Body> {
    stream_pipe: SendStream<Bytes>,
    body: B,
    data_done: bool,
}

impl<B: Body<Data = Bytes>> StreamTask<B> {
    fn new(stream_pipe: SendStream<Bytes>, body: B) -> Self {
        Self {
            stream_pipe,
            body,
            data_done: false,
        }
    }
    async fn drive(&mut self) {
        loop {
            if !self.data_done {
                // we don't have the next chunk of data yet, so just reserve 1 byte to make
                // sure there's some capacity available. h2 will handle the capacity management
                // for the actual body chunk.
                self.stream_pipe.reserve_capacity(1);

                if self.stream_pipe.capacity() == 0 {
                    loop {
                        let cap = poll_fn(|cx| self.stream_pipe.poll_capacity(cx)).await;
                        match cap {
                            Some(Ok(0)) => {}
                            Some(Ok(_)) => break,
                            Some(Err(_e)) => {
                                return;
                                // return Poll::Ready(Err(crate::Error::new_body_write(e)))
                            }
                            None => {
                                // None means the stream is no longer in a
                                // streaming state, we either finished it
                                // somehow, or the remote reset us.
                                // return Poll::Ready(Err(crate::Error::new_body_write(
                                //     "send stream capacity unexpectedly closed",
                                // )));
                                return;
                            }
                        }
                    }
                } else {
                    let stream_rst = poll_fn(|cx| match self.stream_pipe.poll_reset(cx) {
                        std::task::Poll::Pending => std::task::Poll::Ready(false),
                        std::task::Poll::Ready(_) => std::task::Poll::Ready(true),
                    })
                    .await;

                    if stream_rst {
                        // debug!("stream received RST_STREAM: {:?}", reason);
                        // return Poll::Ready(Err(crate::Error::new_body_write(::h2::Error::from(
                        //     reason,
                        // ))));
                        return;
                    }
                }

                match self.body.stream_hint() {
                    StreamHint::None => {
                        let _ = self.stream_pipe.send_data(Bytes::new(), true);
                        self.data_done = true;
                        #[cfg(feature = "logging")]
                        tracing::debug!("H2 Stream task is done ");
                    }
                    StreamHint::Fixed => {
                        if let Some(Ok(data)) = self.body.next_data().await {
                            let _ = self.stream_pipe.send_data(data, true);
                            self.data_done = true;
                        }
                    }
                    StreamHint::Stream => {
                        if let Some(Ok(data)) = self.body.next_data().await {
                            let _ = self.stream_pipe.send_data(data, false);
                        } else {
                            let _ = self.stream_pipe.send_data(Bytes::new(), true);
                            self.data_done = true;
                        }
                    }
                }
            } else {
                let stream_rst = poll_fn(|cx| match self.stream_pipe.poll_reset(cx) {
                    std::task::Poll::Pending => std::task::Poll::Ready(false),
                    std::task::Poll::Ready(_) => std::task::Poll::Ready(true),
                })
                .await;

                if stream_rst {
                    // debug!("stream received RST_STREAM: {:?}", reason);
                    // return Poll::Ready(Err(crate::Error::new_body_write(::h2::Error::from(
                    //     reason,
                    // ))));
                    return;
                }
                break;
                // TODO: Handle trailer
            }
        }
    }
}

pub struct Http2ConnManager<K: Hash + Eq + Display, B> {
    pub req_rx: SingleRecvr<K, B>,
    pub handle: monoio_http::h2::client::SendRequest<bytes::Bytes>,
}

impl<K, B> Http2ConnManager<K, B>
where
    K: Hash + Eq + Display,
    B: Body<Data = Bytes> + From<RecvStream> + 'static,
{
    pub async fn drive(&mut self) {
        while let Some(t) = self.req_rx.req_rx.recv().await {
            let (request, resp_tx, mut connection) = t.parts();
            let (parts, body) = request.into_parts();
            #[cfg(feature = "logging")]
            tracing::debug!("H2 conn manager recv request {:?}", parts);

            let request = http::request::Request::from_parts(parts, ());
            let handle = self.handle.clone();
            let mut ready_handle = handle.ready().await.unwrap();

            let (resp_fut, send_stream) = match ready_handle.send_request(request, false) {
                Ok(ok) => ok,
                Err(e) => {
                    #[cfg(feature = "logging")]
                    tracing::debug!("client send request error: {}", e);
                    let _ = resp_tx.send(Err(e.into()));
                    break;
                }
            };

            #[cfg(feature = "logging")]
            tracing::debug!("H2 conn manager sent request to server");

            monoio::spawn(async move {
                let mut stream_task = StreamTask::new(send_stream, body);
                stream_task.drive().await;
            });

            monoio::spawn(async move {
                match resp_fut.await {
                    Ok(resp) => {
                        let (parts, body) = resp.into_parts();
                        #[cfg(feature = "logging")]
                        tracing::debug!("Response from server {:?}", parts);
                        let ret_resp = Response::from_parts(parts, B::from(body));
                        let _ = resp_tx.send(Ok(ret_resp));
                    }
                    Err(e) => {
                        #[cfg(feature = "logging")]
                        tracing::debug!("Response future returned error {:?}", e);
                        let _ = resp_tx.send(Err(e.into()));
                    }
                }
            });
        }
    }
}

pub fn request_channel<K: Hash + Eq + Display, B>() -> (SingleSender<K, B>, SingleRecvr<K, B>) {
    let (req_tx, req_rx) = mpsc::unbounded::channel();
    (SingleSender { req_tx }, SingleRecvr { req_rx })
}
