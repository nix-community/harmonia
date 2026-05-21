use crate::Algorithm;

/// Read-only access to a hash's algorithm and digest bytes.
///
/// This is the minimal trait needed for formatting (`as_base16`,
/// `as_base32`, etc.). Implemented by both owned and borrowed hash
/// types.
pub trait HashView {
    fn algorithm(&self) -> Algorithm;
    fn digest_bytes(&self) -> &[u8];
}
