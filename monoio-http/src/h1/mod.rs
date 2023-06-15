pub use monoio_codec::FramedRead;

pub mod codec;
pub mod payload;

pub trait BorrowFramedRead {
    type IO;
    type Codec;
    fn framed_mut(&mut self) -> &mut FramedRead<Self::IO, Self::Codec>;
}
