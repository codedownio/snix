/// Given a byte sequence, writes it in escaped form to the passed writer.
/// Does not add surrounding quotes.
///
/// Only five single-byte characters need escaping, so scan and write unescaped runs in bulk.
/// (The previous aho-corasick *stream* API zero-initialized a fresh 64 KiB buffer per call; at
/// ~300k calls per eval that buffer churn was ~49% of instructions on a nixpkgs env eval.)
pub fn write_escaped<P: AsRef<[u8]>>(s: P, w: &mut impl std::io::Write) -> std::io::Result<()> {
    let s = s.as_ref();
    let mut start = 0;
    for (i, b) in s.iter().enumerate() {
        let esc: &[u8] = match b {
            b'\\' => b"\\\\",
            b'\n' => b"\\n",
            b'\r' => b"\\r",
            b'\t' => b"\\t",
            b'"' => b"\\\"",
            _ => continue,
        };
        w.write_all(&s[start..i])?;
        w.write_all(esc)?;
        start = i + 1;
    }
    w.write_all(&s[start..])
}

#[cfg(test)]
mod tests {
    use super::write_escaped;
    use rstest::rstest;

    #[rstest]
    #[case::empty(b"", b"")]
    #[case::doublequote(b"\"", b"\\\"")]
    #[case::colon(b":", b":")]
    #[case::complex(b"foo\n\rbar\\baz", b"foo\\n\\rbar\\\\baz")]
    fn escape(#[case] input: &[u8], #[case] expected: &[u8]) {
        let mut buf = Vec::new();
        write_escaped(input, &mut buf).unwrap();

        assert_eq!(expected, buf.as_slice());
    }
}
