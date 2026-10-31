//! File URI decoding shared by clipboard and folder input.

/// Decode percent-encoded bytes in a file URI as UTF-8.
pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        // Two hex digits after `%`, looked at as bytes: slicing the string
        // there would split a multi-byte letter ("100%ок") and panic.
        if bytes[i] == b'%'
            && let Some(hex) = bytes.get(i + 1..i + 3)
            && let Ok(hex) = std::str::from_utf8(hex)
            && let Ok(b) = u8::from_str_radix(hex, 16)
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
        // A two-byte letter right after `%` or after its first digit stays
        // as it is (slicing the string there used to panic).
        assert_eq!(percent_decode("/tmp/100%ок"), "/tmp/100%ок");
        assert_eq!(percent_decode("/tmp/50%aй"), "/tmp/50%aй");
        assert_eq!(percent_decode("%1ё%D0%9F"), "%1ёП");
        assert_eq!(percent_decode("a%4"), "a%4");
        assert_eq!(percent_decode("%41"), "A");
    }
}
