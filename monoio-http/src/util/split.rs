use std::{cell::UnsafeCell, io, rc::Rc};

use monoio::{
    buf::{IoBuf, IoBufMut, IoVecBuf, IoVecBufMut},
    io::{AsyncReadRent, AsyncWriteRent},
};

/// OwnedReadHalf.
#[derive(Debug)]
pub struct OwnedReadHalf<T>(Rc<UnsafeCell<T>>);

/// OwnedWriteHalf.
#[derive(Debug)]
pub struct OwnedWriteHalf<T>(Rc<UnsafeCell<T>>);

/// Split an io into read half and write half.
/// # Safety
/// User must make sure the io r/w can be done at the same time.
pub unsafe fn split<T>(io: T) -> (OwnedReadHalf<T>, OwnedWriteHalf<T>) {
    let shared = Rc::new(UnsafeCell::new(io));
    (
        OwnedReadHalf(shared.clone()),
        OwnedWriteHalf(shared),
    )
}

impl<IO> AsyncReadRent for OwnedReadHalf<IO>
where
    IO: AsyncReadRent,
{
    type ReadFuture<'a, B> = impl std::future::Future<Output = monoio::BufResult<usize, B>> where
        B: 'a, Self: 'a;
    type ReadvFuture<'a, B> = impl std::future::Future<Output = monoio::BufResult<usize, B>> where
        B: 'a, Self: 'a;

    fn read<T: IoBufMut>(&mut self, buf: T) -> Self::ReadFuture<'_, T> {
        // Submit the read operation
        let raw_stream = unsafe { &mut *self.0.get() };
        raw_stream.read(buf)
    }

    fn readv<T: IoVecBufMut>(&mut self, buf: T) -> Self::ReadvFuture<'_, T> {
        // Submit the read operation
        let raw_stream = unsafe { &mut *self.0.get() };
        raw_stream.readv(buf)
    }
}

impl<IO> AsyncWriteRent for OwnedWriteHalf<IO>
where
    IO: AsyncWriteRent,
{
    type WriteFuture<'a, B> = impl std::future::Future<Output = monoio::BufResult<usize, B>> where
        B: 'a, Self: 'a;
    type WritevFuture<'a, B> = impl std::future::Future<Output = monoio::BufResult<usize, B>> where
        B: 'a, Self: 'a;
    type FlushFuture<'a> = impl std::future::Future<Output = io::Result<()>> where Self: 'a;
    type ShutdownFuture<'a> = impl std::future::Future<Output = io::Result<()>> where Self: 'a;

    fn write<T: IoBuf>(&mut self, buf: T) -> Self::WriteFuture<'_, T> {
        // Submit the write operation
        let raw_stream = unsafe { &mut *self.0.get() };
        raw_stream.write(buf)
    }

    fn writev<T: IoVecBuf>(&mut self, buf_vec: T) -> Self::WritevFuture<'_, T> {
        let raw_stream = unsafe { &mut *self.0.get() };
        raw_stream.writev(buf_vec)
    }

    fn flush(&mut self) -> Self::FlushFuture<'_> {
        // Tcp stream does not need flush.
        async move { Ok(()) }
    }

    fn shutdown(&mut self) -> Self::ShutdownFuture<'_> {
        let raw_stream = unsafe { &mut *self.0.get() };
        raw_stream.shutdown()
    }
}
