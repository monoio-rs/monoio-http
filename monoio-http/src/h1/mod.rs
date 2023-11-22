pub use monoio_codec::FramedRead;

pub mod codec;
pub mod payload;

pub trait BorrowFramedRead {
    type IO;
    type Codec;
    fn framed_mut(&mut self) -> &mut FramedRead<Self::IO, Self::Codec>;
}

impl<S: ?Sized + BorrowFramedRead> BorrowFramedRead for &mut S {
    type IO = S::IO;
    type Codec = S::Codec;

    fn framed_mut(&mut self) -> &mut FramedRead<Self::IO, Self::Codec> {
        (**self).framed_mut()
    }
}
