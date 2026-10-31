//! File URI decoding shared by clipboard and folder input.

/// Decode percent-encoded bytes in a file URI as UTF-8.
pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(b);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::percent_decode;

    #[test]
    fn file_uri_bytes() {
        assert_eq!(percent_decode("/tmp/a%20b"), "/tmp/a b");
        assert_eq!(percent_decode("%D0%9F"), "П");
        assert_eq!(percent_decode("a+b%2Fc"), "a+b/c");
        assert_eq!(percent_decode("%ZZ%2%"), "%ZZ%2%");
        assert_eq!(percent_decode("%FF"), "�");
    }
}
