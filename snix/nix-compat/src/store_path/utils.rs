use std::fmt::Display;

use super::{ParseStorePathError, STORE_DIR, StorePathRef};
use crate::derivation::OutputName;
use crate::nixhash::{HashAlgo, NixHash, Sha256};
use crate::{format_sha256, nixbase32};

/// compress_hash takes an arbitrarily long sequence of bytes (usually
/// a hash digest), and returns a sequence of bytes of length
/// OUTPUT_SIZE.
///
/// It's calculated by rotating through the bytes in the output buffer
/// (zero- initialized), and XOR'ing with each byte of the passed
/// input. It consumes 1 byte at a time, and XOR's it with the current
/// value in the output buffer.
///
/// This mimics equivalent functionality in C++ Nix.
pub fn compress_hash<const OUTPUT_SIZE: usize>(input: &[u8]) -> [u8; OUTPUT_SIZE] {
    let mut output = [0; OUTPUT_SIZE];

    for (ii, ch) in input.iter().enumerate() {
        output[ii % OUTPUT_SIZE] ^= ch;
    }

    output
}

/// This builds a store path, for a CAHash::Text type store path.
/// If you don't want to have to pass the entire contents,
/// you might want to use [build_text_path_from_content_digest] instead.
pub fn build_text_path<'r, 'name>(
    name: &'name str,
    content: impl AsRef<[u8]>,
    references: impl IntoIterator<Item = StorePathRef<'r>> + 'r,
) -> Result<StorePathRef<'name>, ParseStorePathError> {
    build_text_path_from_content_digest(name, Sha256::digest_bytes(content.as_ref()), references)
}

/// This builds a store path, for a CAHash::Text type store path.
/// `content_digest` needs to be the sha256 digest of the contents.
/// If you have the contents as a byte slice, you can also use [build_text_path].
pub fn build_text_path_from_content_digest<'r, 'n>(
    name: &'n str,
    content_digest: impl Into<Sha256>,
    references: impl IntoIterator<Item = StorePathRef<'r>> + 'r,
) -> Result<StorePathRef<'n>, ParseStorePathError> {
    // produce the sha256 digest of the contents

    let ty = format_references("text", references, false);

    build_store_path_from_fingerprint_parts(ty, &content_digest.into(), name)
}

/// This builds a store path for a content-addressed path (used for fetches and FODs).
pub fn build_ca_path<'r, 'n>(
    name: &'n str,
    is_recursive: bool,
    hash: &NixHash,
    references: impl IntoIterator<Item = StorePathRef<'r>> + 'r,
    has_self_ref: bool,
) -> Result<StorePathRef<'n>, ParseStorePathError> {
    let inner_digest = if let NixHash::Sha256(digest) = hash
        && is_recursive
    {
        Sha256::new(*digest)
    } else {
        fod_digest(is_recursive, hash, None)
    };

    if hash.algo() == HashAlgo::Sha256 && is_recursive {
        build_store_path_from_fingerprint_parts(
            format_references("source", references, has_self_ref),
            &inner_digest,
            name,
        )
    } else {
        // FUTUREWORK: dump when references are non-empty, and when has_self_ref is true.
        // Add an assertion here?
        build_store_path_from_fingerprint_parts("output:out", &inner_digest, name)
    }
}

/// Builds an input-addressed store path.
///
/// Input-addresed store paths are always derivation outputs, the "input" in question is the
/// derivation and its closure.
pub fn build_output_path<'n>(
    name: &'n str,
    hash_derivation_modulo: &Sha256,
    output_name: &OutputName,
) -> Result<StorePathRef<'n>, ParseStorePathError> {
    build_store_path_from_fingerprint_parts(
        format_args!("output:{output_name}"),
        hash_derivation_modulo,
        name,
    )
}

/// This builds a store path from fingerprint parts.
///
/// This is called from [build_text_path], [build_output_path] and
/// [build_ca_path] to assemble the final path.
///
/// Using the inputs, it creates a fingerprint, hashes and compresses it,
/// then uses the passed `name` to create a store path.
///
/// If that `name` doesn't match store path name requirements, the error is
/// passed along.
fn build_store_path_from_fingerprint_parts<'n>(
    ty: impl Display,
    inner_digest: &Sha256,
    name: &'n str,
) -> Result<StorePathRef<'n>, ParseStorePathError> {
    let fingerprint_hash = format_sha256!("{ty}:sha256:{inner_digest:x}:{STORE_DIR}:{name}");
    // name validation happens in here.
    StorePathRef::from_name_and_digest_fixed(name, compress_hash(&fingerprint_hash))
}

pub(crate) fn fod_digest(
    is_recursive: bool,
    hash: &NixHash,
    out_output_path: Option<StorePathRef<'_>>,
) -> Sha256 {
    let absolute_sp_optional = std::fmt::from_fn(|f| {
        if let Some(sp) = &out_output_path {
            write!(f, "{}", sp.as_absolute_path_fmt())?
        }
        Ok(())
    });

    if is_recursive {
        format_sha256!(
            "fixed:out:r:{}:{}",
            hash.as_nix_lowerhex_string_fmt(),
            absolute_sp_optional
        )
    } else {
        format_sha256!(
            "fixed:out:{}:{}",
            hash.as_nix_lowerhex_string_fmt(),
            absolute_sp_optional
        )
    }
}

/// This contains the Nix logic to create "reference strings", used for the
/// output path calculation of ca paths and text paths.
fn format_references<'a, R>(ty: &'a str, references: R, has_self_ref: bool) -> impl Display + 'a
where
    R: IntoIterator<Item = StorePathRef<'a>> + 'a,
{
    struct ReferencesFormatter<'a, R> {
        ty: &'a str,
        references: std::cell::RefCell<R>,
        has_self_ref: bool,
    }

    impl<'a, R> Display for ReferencesFormatter<'a, R>
    where
        R: Iterator<Item = StorePathRef<'a>> + 'a,
    {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.ty)?;
            while let Some(reference) = self.references.borrow_mut().next() {
                write!(f, ":{}", reference.as_absolute_path_fmt()).unwrap();
            }

            if self.has_self_ref {
                write!(f, ":self")?;
            }

            Ok(())
        }
    }

    ReferencesFormatter {
        ty,
        references: std::cell::RefCell::new(references.into_iter()),
        has_self_ref,
    }
}

/// Nix placeholders (i.e. values returned by `builtins.placeholder`)
/// are used to populate outputs with paths that must be
/// string-replaced with the actual placeholders later, at runtime.
///
/// The actual placeholder is basically just a SHA256 hash encoded in
/// cppnix format.
pub fn hash_placeholder(name: &str) -> String {
    format!(
        "/{}",
        nixbase32::encode(&format_sha256!("nix-output:{name}"))
    )
}

#[cfg(test)]
mod test {
    use hex_literal::hex;

    use super::*;
    use crate::{nixhash::NixHash, store_path::StorePathRef};

    #[test]
    fn build_text_path_with_zero_references() {
        // This hash should match `builtins.toFile`, e.g.:
        //
        // nix-repl> builtins.toFile "foo" "bar"
        // "/nix/store/vxjiwkjkn7x4079qvh1jkl5pn05j2aw0-foo"

        let store_path: StorePathRef =
            build_text_path("foo", "bar", []).expect("build_store_path() should succeed");

        assert_eq!(
            store_path.to_absolute_path().as_str(),
            "/nix/store/vxjiwkjkn7x4079qvh1jkl5pn05j2aw0-foo"
        );
    }

    #[test]
    fn build_text_path_with_non_zero_references() {
        // This hash should match:
        //
        // nix-repl> builtins.toFile "baz" "${builtins.toFile "foo" "bar"}"
        // "/nix/store/5xd714cbfnkz02h2vbsj4fm03x3f15nf-baz"

        let inner: StorePathRef =
            build_text_path("foo", "bar", []).expect("path_with_references() should succeed");

        let outer: StorePathRef = build_text_path("baz", inner.to_absolute_path(), [inner])
            .expect("path_with_references() should succeed");

        assert_eq!(
            outer.to_absolute_path().as_str(),
            "/nix/store/5xd714cbfnkz02h2vbsj4fm03x3f15nf-baz"
        );
    }

    #[test]
    fn build_sha1_path() {
        let outer: StorePathRef = build_ca_path(
            "bar",
            true,
            &NixHash::Sha1(hex!("0beec7b5ea3f0fdbc95d0dd47f3c5bc275da8a33")),
            [],
            false,
        )
        .expect("path_with_references() should succeed");

        assert_eq!(
            outer.to_absolute_path().as_str(),
            "/nix/store/mp57d33657rf34lzvlbpfa1gjfv5gmpg-bar"
        );
    }

    #[test]
    fn build_store_path_with_non_zero_references() {
        // This hash should match:
        //
        // nix-repl> builtins.toFile "baz" "${builtins.toFile "foo" "bar"}"
        // "/nix/store/5xd714cbfnkz02h2vbsj4fm03x3f15nf-baz"
        //
        // $ nix store make-content-addressed /nix/store/5xd714cbfnkz02h2vbsj4fm03x3f15nf-baz
        // rewrote '/nix/store/5xd714cbfnkz02h2vbsj4fm03x3f15nf-baz' to '/nix/store/s89y431zzhmdn3k8r96rvakryddkpv2v-baz'
        let outer: StorePathRef = build_ca_path(
            "baz",
            true,
            &NixHash::Sha256(
                nixbase32::decode(b"1xqkzcb3909fp07qngljr4wcdnrh1gdam1m2n29i6hhrxlmkgkv1")
                    .expect("nixbase32 should decode")
                    .try_into()
                    .expect("should have right len"),
            ),
            [
                StorePathRef::from_bytes(b"dxwkwjzdaq7ka55pkk252gh32bgpmql4-foo")
                    .expect("to parse"),
            ],
            false,
        )
        .expect("path_with_references() should succeed");

        assert_eq!(
            outer.to_absolute_path().as_str(),
            "/nix/store/s89y431zzhmdn3k8r96rvakryddkpv2v-baz"
        );
    }
}
