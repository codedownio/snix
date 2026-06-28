use std::fmt;
use std::io::Write;

use data_encoding::{HEXLOWER, HEXUPPER};
use sha2::Digest;

use crate::nixhash::HashAlgo;

use super::NixHash;

type Sha256Array = [u8; Sha256::digest_length()];

/// A SHA256 hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Sha256(Sha256Array);
impl Sha256 {
    pub const fn digest_length() -> usize {
        HashAlgo::Sha256.digest_length()
    }

    pub const fn new(digest: Sha256Array) -> Self {
        Self(digest)
    }

    /// Shorthand for digesting a byte slice.
    ///
    /// # Examples
    /// ```
    /// use nix_compat::nixhash::Sha256;
    ///
    /// let sha256 = Sha256::digest_bytes("abc");
    /// assert_eq!(format!("{sha256:x}"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    /// ```
    pub fn digest_bytes<C: AsRef<[u8]>>(content: C) -> Self {
        let mut w = Sha256Digester::new();
        w.update(content.as_ref());
        w.finalize()
    }

    /// Hashes formatted string data with SHA-256, without an intermediate buffer.
    ///
    /// Analogous to [`std::fmt::format`].
    ///
    /// # Examples
    /// ```
    /// use nix_compat::nixhash::Sha256;
    ///
    /// let sha256 = Sha256::digest_display(format_args!("{}bc", "a"));
    /// assert_eq!(format!("{sha256:x}"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    /// ```
    pub fn digest_display<D: fmt::Display>(value: D) -> Self {
        let mut w = Sha256Digester::new();
        write!(&mut w, "{value}").unwrap();
        w.finalize()
    }

    pub const fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub const fn into_bytes(self) -> Sha256Array {
        self.0
    }
}

impl fmt::LowerHex for Sha256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", HEXLOWER.encode_display(self.as_bytes()))
    }
}

impl fmt::UpperHex for Sha256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", HEXUPPER.encode_display(self.as_bytes()))
    }
}

impl AsRef<[u8]> for Sha256 {
    fn as_ref(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl std::borrow::Borrow<[u8]> for Sha256 {
    fn borrow(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl std::ops::Deref for Sha256 {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.as_bytes()
    }
}

impl From<Sha256Array> for Sha256 {
    fn from(digest: Sha256Array) -> Self {
        Sha256::new(digest)
    }
}

impl From<Sha256> for Sha256Array {
    fn from(value: Sha256) -> Self {
        value.into_bytes()
    }
}

impl From<Sha256> for NixHash {
    fn from(value: Sha256) -> Self {
        NixHash::Sha256(value.into_bytes())
    }
}

impl PartialEq<Sha256Array> for Sha256 {
    fn eq(&self, other: &Sha256Array) -> bool {
        &self.0 == other
    }
}

impl PartialEq<Sha256> for Sha256Array {
    fn eq(&self, other: &Sha256) -> bool {
        self == &other.0
    }
}

impl PartialEq<[u8]> for Sha256 {
    fn eq(&self, other: &[u8]) -> bool {
        self.0 == other
    }
}

impl PartialEq<Sha256> for [u8] {
    fn eq(&self, other: &Sha256) -> bool {
        self == other.0
    }
}

impl PartialEq<NixHash> for Sha256 {
    fn eq(&self, other: &NixHash) -> bool {
        matches!(other, NixHash::Sha256(sha256) if self == sha256)
    }
}

impl PartialEq<Sha256> for NixHash {
    fn eq(&self, other: &Sha256) -> bool {
        matches!(self, NixHash::Sha256(sha256) if other == sha256)
    }
}

/// A digester that takes in bytes and ultimately produces a [`Sha256`].
///
/// # Examples
/// ```
/// use nix_compat::nixhash::{Sha256, Sha256Digester};
///
/// let one_shot = Sha256::digest_bytes("hello, world");
///
/// let mut ctx = Sha256Digester::new();
/// ctx.update("hello");
/// ctx.update(", ");
/// ctx.update("world");
/// let multi_path = ctx.finalize();
///
/// assert_eq!(one_shot, multi_path);
/// ```
pub struct Sha256Digester(sha2::Sha256);
impl Sha256Digester {
    /// Returns a new digester
    pub fn new() -> Self {
        Self(sha2::Sha256::new())
    }

    /// Updates the digester with the provided bytes
    pub fn update<C: AsRef<[u8]>>(&mut self, data: C) {
        self.0.update(data);
    }

    /// Finalize the digest and return the produced [`Sha256`].
    pub fn finalize(self) -> Sha256 {
        let digest: [u8; HashAlgo::Sha256.digest_length()] = self.0.finalize().into();
        Sha256::new(digest)
    }
}

impl Default for Sha256Digester {
    fn default() -> Self {
        Self::new()
    }
}

impl Write for Sha256Digester {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.update(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Analogous to [`std::format`], but returning only the SHA-256 digest of the formatted string.
///
/// # Examples
/// ```
/// use nix_compat::format_sha256;
/// let sha256 = format_sha256!("{}bc", "a");
/// assert_eq!(format!("{sha256:x}"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
/// ```
#[macro_export]
macro_rules! format_sha256 {
    ($($args:tt)*) => {
        ::nix_compat::nixhash::Sha256::digest_display(format_args!($($args)*))
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_literal::hex;

    struct Sha256Formats {
        input: &'static str,
        hash: Sha256,
        base16: &'static str,
    }

    /// value taken from: https://tools.ietf.org/html/rfc4634
    const SHA256_ABC: Sha256Formats = Sha256Formats {
        input: "abc",
        hash: Sha256::new(hex!(
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        )),
        base16: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
    };

    /// value taken from: https://tools.ietf.org/html/rfc4634
    const SHA256_LONG: Sha256Formats = Sha256Formats {
        input: "abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
        hash: Sha256::new(hex!(
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        )),
        base16: "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
    };

    #[rstest_reuse::template]
    #[rstest::rstest]
    #[case::abc(SHA256_ABC)]
    #[case::long(SHA256_LONG)]
    fn hash_formats(#[case] hash: Sha256Formats) {}

    /// Test `fmt::LowerHex` implementation
    #[rstest_reuse::apply(hash_formats)]
    fn lower_hex(#[case] hash: Sha256Formats) {
        let actual = format!("{:x}", hash.hash);
        assert_eq!(hash.base16, actual);
    }

    /// Test `fmt::UpperHex` implementation
    #[rstest_reuse::apply(hash_formats)]
    fn upper_hex(#[case] hash: Sha256Formats) {
        let expected = hash.base16.to_uppercase();
        let actual = format!("{:X}", hash.hash);
        assert_eq!(expected, actual);
    }

    /// Test `Sha256::digest_bytes` implementation
    #[rstest_reuse::apply(hash_formats)]
    fn digest_bytes(#[case] hash: Sha256Formats) {
        let actual = Sha256::digest_bytes(hash.input);
        assert_eq!(hash.hash, actual);
    }

    /// Test `Sha256::digest_display` implementation
    #[rstest_reuse::apply(hash_formats)]
    fn digest_display(#[case] hash: Sha256Formats) {
        let actual = Sha256::digest_display(format_args!("{}", hash.input));
        assert_eq!(hash.hash, actual);
    }

    /// Test `format_sha256` macro implementation
    #[rstest_reuse::apply(hash_formats)]
    fn format_sha256_test(#[case] hash: Sha256Formats) {
        let actual = format_sha256!("{}", hash.input);
        assert_eq!(hash.hash, actual);
    }

    /// Test `Sha256Digester` implementation
    #[rstest_reuse::apply(hash_formats)]
    fn digester(#[case] hash: Sha256Formats) {
        let mut ctx = Sha256Digester::new();
        ctx.update(hash.input);
        let actual = ctx.finalize();
        assert_eq!(hash.hash, actual);
    }

    /// Test `Sha256Digester` `std::io::Write` implementation
    #[rstest_reuse::apply(hash_formats)]
    fn digester_write(#[case] hash: Sha256Formats) {
        let mut ctx = Sha256Digester::new();
        ctx.write_all(hash.input.as_bytes()).unwrap();
        let actual = ctx.finalize();
        assert_eq!(hash.hash, actual);
    }
}
