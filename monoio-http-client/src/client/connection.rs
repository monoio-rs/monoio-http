use std::{future::poll_fn, task::Poll};

use bytes::Bytes;
use monoio::io::{sink::SinkExt, stream::Stream, AsyncReadRent, AsyncWriteRent, Split};
use monoio_http::{
    common::{
        body::{Body, HttpBody, StreamHint},
        error::HttpError,
        request::Request,
        response::Response,
    },
    h1::{
        codec::{decoder::DecodeError, ClientCodec},
        payload::FramedPayloadRecvr,
    },
    h2::{client::SendRequest, SendStream},
};

use crate::Error;

pub struct StreamBodyTask<B: Body> {
    stream_pipe: SendStream<Bytes>,
    body: B,
    data_done: bool,
}

impl<B: Body<Data = Bytes>> StreamBodyTask<B> {
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
                // sure there's some capacity available. H2 will handle the capacity management
                // for the actual body chunk.
                self.stream_pipe.reserve_capacity(1);

                if self.stream_pipe.capacity() == 0 {
                    loop {
                        let cap = poll_fn(|cx| self.stream_pipe.poll_capacity(cx)).await;
                        match cap {
                            Some(Ok(0)) => {}
                            Some(Ok(_)) => break,
                            Some(Err(_e)) => {
                                #[cfg(feature = "logging")]
                                tracing::error!("H2 StreamBodyTask {_e:?}");
                                return;
                            }
                            None => {
                                // None means the stream is no longer in a
                                // streaming state.
                                #[cfg(feature = "logging")]
                                tracing::error!(
                                    "H2 StreamBodyTask Send stream capacity unexpectedly closed",
                                );
                                return;
                            }
                        }
                    }
                } else {
                    let stream_rst = poll_fn(|cx| match self.stream_pipe.poll_reset(cx) {
                        Poll::Pending => Poll::Ready(false),
                        Poll::Ready(_) => Poll::Ready(true),
                    })
                    .await;

                    if stream_rst {
                        #[cfg(feature = "logging")]
                        tracing::error!("H2 StreamBodyTask stream reset");
                        return;
                    }
                }

                match self.body.stream_hint() {
                    StreamHint::None => {
                        let _ = self.stream_pipe.send_data(Bytes::new(), true);
                        self.data_done = true;
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
                    Poll::Pending => Poll::Ready(false),
                    Poll::Ready(_) => Poll::Ready(true),
                })
                .await;

                if stream_rst {
                    #[cfg(feature = "logging")]
                    tracing::error!("H2 StreamBodyTask stream reset");
                    return;
                }

                // TODO: Handle trailer
                break;
            }
        }
    }
}

pub enum HttpConnection<IO: AsyncReadRent + AsyncWriteRent + Split> {
    H1(Option<ClientCodec<IO>>),
    H2(SendRequest<Bytes>),
}

impl<IO: AsyncReadRent + AsyncWriteRent + Split> HttpConnection<IO> {
    pub async fn send_request<B>(
        &mut self,
        request: Request<B>,
    ) -> (crate::Result<Response<HttpBody>>, bool)
    where
        B: Body<Data = Bytes, Error = HttpError> + 'static,
    {
        match self {
            Self::H1(ref mut handle) => {
                let mut h = match handle.take() {
                    Some(h) => h,
                    None => return (Err(Error::MissingCodec), false),
                };

                if let Err(e) = h.send_and_flush(request).await {
                    #[cfg(feature = "logging")]
                    tracing::error!("send upstream request error {:?}", e);
                    return (Err(e.into()), false);
                }

                match h.next().await {
                    Some(Ok(resp)) => {
                        let (data_tx, data_rx) = local_sync::mpsc::unbounded::channel();
                        let (parts, body_builder) = resp.into_parts();
                        let mut framed_payload = body_builder.with_io(h);
                        let framed_payload_rcvr = FramedPayloadRecvr {
                            data_rx,
                            hint: framed_payload.stream_hint(),
                        };

                        while let Some(r) = framed_payload.next_data().await {
                            let _ = data_tx.send(Some(r));
                        }
                        *handle = Some(framed_payload.get_source());
                        
                        let resp = Response::from_parts(parts, framed_payload_rcvr.into());
                        (Ok(resp), false)
                    }
                    Some(Err(e)) => {
                        #[cfg(feature = "logging")]
                        tracing::error!("decode upstream response error {:?}", e);
                        (Err(e.into()), false)
                    }
                    None => {
                        #[cfg(feature = "logging")]
                        tracing::error!("upstream return eof");
                        (Err(DecodeError::UnexpectedEof.into()), false)
                    }
                }
            }

            Self::H2(h) => {
                let (parts, body) = request.into_parts();
                #[cfg(feature = "logging")]
                tracing::debug!("H2 conn manager Request:{:?}", parts);
                let request = http::request::Request::from_parts(parts, ());

                let handle = h.clone();
                let mut ready_handle = match handle.ready().await {
                    Ok(r) => r,
                    Err(e) => {
                        #[cfg(feature = "logging")]
                        tracing::error!("H2 conn manager ready error: {e:?}");
                        return (Err(e.into()), true);
                    }
                };

                let (resp_fut, send_stream) = match ready_handle.send_request(request, false) {
                    Ok(ok) => ok,
                    Err(e) => {
                        #[cfg(feature = "logging")]
                        tracing::debug!("client send request error: {e}");
                        return (Err(e.into()), false);
                    }
                };

                monoio::spawn(async move {
                    let mut stream_task = StreamBodyTask::new(send_stream, body);
                    stream_task.drive().await;
                });

                match resp_fut.await {
                    Ok(resp) => {
                        #[cfg(feature = "logging")]
                        tracing::debug!("H2 Conn Response:");
                        (Ok(HttpBody::response(resp)), false)
                    }
                    Err(e) => {
                        #[cfg(feature = "logging")]
                        tracing::debug!("H2 conn Response error {e:?}");
                        (Err(e.into()), false)
                    }
                }
            }
        }
    }

    pub fn is_http2(&self) -> bool {
        match self {
            Self::H1(_) => false,
            Self::H2(_) => true,
        }
    }

    pub fn http2_conn_clone(&self) -> Self {
        match self {
            Self::H1(_) => unreachable!(),
            Self::H2(h) => Self::H2(h.clone()),
        }
    }
}
