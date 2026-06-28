use std::io::Write;

use sha2::Digest;

use crate::nixhash::{HashAlgo, Sha256Digester};

use super::NixHash;

enum Inner {
    Md5(md5::Md5),
    Sha1(sha1::Sha1),
    Sha256(Sha256Digester),
    Sha512(sha2::Sha512),
}

/// A digester that takes in bytes and ultimately produces a [`NixHash`].
///
/// # Examples
/// ```
/// use nix_compat::nixhash::{HashAlgo, NixHashDigester};
///
/// let one_shot = HashAlgo::Sha256.digest_bytes("hello, world");
///
/// let mut ctx = NixHashDigester::new(HashAlgo::Sha256);
/// ctx.update("hello");
/// ctx.update(", ");
/// ctx.update("world");
/// let multi_path = ctx.finalize();
///
/// assert_eq!(one_shot, multi_path);
/// ```
pub struct NixHashDigester(Inner);

impl NixHashDigester {
    /// Returns a new digester for the specified [`HashAlgo`].
    pub fn new(algo: HashAlgo) -> Self {
        match algo {
            HashAlgo::Md5 => Self(Inner::Md5(md5::Md5::new())),
            HashAlgo::Sha1 => Self(Inner::Sha1(sha1::Sha1::new())),
            HashAlgo::Sha256 => Self(Inner::Sha256(Sha256Digester::new())),
            HashAlgo::Sha512 => Self(Inner::Sha512(sha2::Sha512::new())),
        }
    }

    /// Returns the hash algorithm that this digester uses.
    pub fn algorithm(&self) -> HashAlgo {
        match self.0 {
            Inner::Md5(_) => HashAlgo::Md5,
            Inner::Sha1(_) => HashAlgo::Sha1,
            Inner::Sha256(_) => HashAlgo::Sha256,
            Inner::Sha512(_) => HashAlgo::Sha512,
        }
    }

    /// Updates the digester with the provided bytes.
    pub fn update<C: AsRef<[u8]>>(&mut self, data: C) {
        match &mut self.0 {
            Inner::Md5(d) => d.update(data),
            Inner::Sha1(d) => d.update(data),
            Inner::Sha256(d) => d.update(data),
            Inner::Sha512(d) => d.update(data),
        }
    }

    /// Finalize the digest and return the produced [`NixHash`].
    pub fn finalize(self) -> NixHash {
        match self.0 {
            Inner::Md5(d) => {
                let digest: [u8; HashAlgo::Md5.digest_length()] = d.finalize().into();
                NixHash::Md5(digest)
            }
            Inner::Sha1(d) => {
                let digest: [u8; HashAlgo::Sha1.digest_length()] = d.finalize().into();
                NixHash::Sha1(digest)
            }
            Inner::Sha256(d) => d.finalize().into(),
            Inner::Sha512(d) => {
                let digest: [u8; HashAlgo::Sha512.digest_length()] = d.finalize().into();
                NixHash::Sha512(Box::new(digest))
            }
        }
    }
}

impl Write for NixHashDigester {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.update(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl HashAlgo {
    /// Shorthand for digesting a byte slice.
    ///
    /// # Examples
    /// ```
    /// use nix_compat::nixhash::HashAlgo;
    ///
    /// let sha256 = HashAlgo::Sha256.digest_bytes("abc");
    /// assert_eq!(sha256.to_nix_nixbase32(), "sha256:1b8m03r63zqhnjf7l5wnldhh7c134ap5vpj0850ymkq1iyzicy5s");
    /// ```
    pub fn digest_bytes<C: AsRef<[u8]>>(&self, content: C) -> NixHash {
        let mut c = NixHashDigester::new(*self);
        c.update(content);
        c.finalize()
    }

    /// Hashes an agument implementing display with this algorithm, without an intermediate buffer.
    ///
    /// Analogous to [`std::fmt::format`].
    ///
    /// # Examples
    /// ```
    /// use nix_compat::nixhash::HashAlgo;
    ///
    /// let sha256 = HashAlgo::Sha256.digest_display(format_args!("{}bc", "a"));
    /// assert_eq!(sha256.to_nix_nixbase32(), "sha256:1b8m03r63zqhnjf7l5wnldhh7c134ap5vpj0850ymkq1iyzicy5s");
    /// ```
    pub fn digest_display<D: std::fmt::Display>(&self, value: D) -> NixHash {
        let mut c = NixHashDigester::new(*self);
        write!(c, "{value}").unwrap();
        c.finalize()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::LazyLock;

    use super::*;
    use hex_literal::hex;

    struct HashFormats {
        input: &'static str,
        algo: HashAlgo,
        hash: NixHash,
    }

    /// value taken from: https://tools.ietf.org/html/rfc1321
    const MD5_EMPTY: HashFormats = HashFormats {
        input: "",
        algo: HashAlgo::Md5,
        hash: NixHash::Md5(hex!("d41d8cd98f00b204e9800998ecf8427e")),
    };

    /// value taken from: https://tools.ietf.org/html/rfc1321
    const MD5_ABC: HashFormats = HashFormats {
        input: "abc",
        algo: HashAlgo::Md5,
        hash: NixHash::Md5(hex!("900150983cd24fb0d6963f7d28e17f72")),
    };

    /// value taken from: https://tools.ietf.org/html/rfc3174
    const SHA1_ABC: HashFormats = HashFormats {
        input: "abc",
        algo: HashAlgo::Sha1,
        hash: NixHash::Sha1(hex!("a9993e364706816aba3e25717850c26c9cd0d89d")),
    };

    /// value taken from: https://tools.ietf.org/html/rfc3174
    const SHA1_LONG: HashFormats = HashFormats {
        input: "abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
        algo: HashAlgo::Sha1,
        hash: NixHash::Sha1(hex!("84983e441c3bd26ebaae4aa1f95129e5e54670f1")),
    };

    /// value taken from: https://tools.ietf.org/html/rfc4634
    const SHA256_ABC: HashFormats = HashFormats {
        input: "abc",
        algo: HashAlgo::Sha256,
        hash: NixHash::Sha256(hex!(
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        )),
    };

    /// value taken from: https://tools.ietf.org/html/rfc4634
    const SHA256_LONG: HashFormats = HashFormats {
        input: "abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
        algo: HashAlgo::Sha256,
        hash: NixHash::Sha256(hex!(
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        )),
    };

    /// value taken from: https://tools.ietf.org/html/rfc4634
    static SHA512_ABC: LazyLock<HashFormats> = LazyLock::new(|| HashFormats {
        input: "abc",
        algo: HashAlgo::Sha512,
        hash: NixHash::Sha512(Box::new(hex!(
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
        ))),
    });

    /// value taken from: https://tools.ietf.org/html/rfc4634
    static SHA512_LONG: LazyLock<HashFormats> = LazyLock::new(|| HashFormats {
        input: "abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu",
        algo: HashAlgo::Sha512,
        hash: NixHash::Sha512(Box::new(hex!(
            "8e959b75dae313da8cf4f72814fc143f8f7779c6eb9f7fa17299aeadb6889018501d289e4900f7e4331b99dec4b5433ac7d329eeb6dd26545e96e55b874be909"
        ))),
    });

    #[rstest_reuse::template]
    #[rstest::rstest]
    #[case::md5_empty(&MD5_EMPTY)]
    #[case::md5_abc(&MD5_ABC)]
    #[case::sha1_abc(&SHA1_ABC)]
    #[case::sha1_long(&SHA1_LONG)]
    #[case::sha256_abc(&SHA256_ABC)]
    #[case::sha256_long(&SHA256_LONG)]
    #[case::sha512_abc(&*SHA512_ABC)]
    #[case::sha512_long(&*SHA512_LONG)]
    fn hash_formats(#[case] hash: &HashFormats) {}

    /// Test `HashAlgo::digest_bytes` implementation
    #[rstest_reuse::apply(hash_formats)]
    fn digest_bytes(#[case] hash: &HashFormats) {
        let actual = hash.algo.digest_bytes(hash.input);
        assert_eq!(hash.hash, actual);
    }

    /// Test `HashAlgo::digest_display` implementation
    #[rstest_reuse::apply(hash_formats)]
    fn digest_display(#[case] hash: &HashFormats) {
        let actual = hash.algo.digest_display(format_args!("{}", hash.input));
        assert_eq!(hash.hash, actual);
    }

    /// Test `NixHashDigester` implementation
    #[rstest_reuse::apply(hash_formats)]
    fn digester(#[case] hash: &HashFormats) {
        let mut ctx = NixHashDigester::new(hash.algo);
        ctx.update(hash.input);
        let actual = ctx.finalize();
        assert_eq!(hash.hash, actual);
    }

    /// Test `NixHashDigester` `std::io::Write` implementation
    #[rstest_reuse::apply(hash_formats)]
    fn digester_write(#[case] hash: &HashFormats) {
        let mut ctx = NixHashDigester::new(hash.algo);
        ctx.write_all(hash.input.as_bytes()).unwrap();
        let actual = ctx.finalize();
        assert_eq!(hash.hash, actual);
    }
}
