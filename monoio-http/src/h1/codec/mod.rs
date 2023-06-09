mod client;
mod server;

pub mod decoder;
pub mod encoder;
pub use client::ClientCodec;
pub use server::ServerCodec;

fn byte_case_insensitive_cmp(user_data: &[u8], lower_case_target: &'static [u8]) -> bool {
    if user_data.len() != lower_case_target.len() {
        return false;
    }
    user_data
        .iter()
        .zip(lower_case_target.iter())
        .all(|(&a, &b)| a == b || (a >= b'A' && a <= b'Z' && a + 32 == b))
}
