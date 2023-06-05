mod decoder;
mod encoder;
pub(crate) mod header;
pub(crate) mod huffman;
mod table;

#[cfg(test)]
mod test;

pub use self::{
    decoder::{Decoder, DecoderError, NeedMore},
    encoder::Encoder,
    header::{BytesStr, Header},
};
