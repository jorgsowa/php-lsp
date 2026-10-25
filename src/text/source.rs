//! Decoding PHP files read from disk.

/// Decode file bytes as UTF-8, substituting U+FFFD for invalid sequences so
/// legacy (e.g. Latin-1) files are still indexed instead of skipped.
#[cfg_attr(target_family = "wasm", allow(dead_code))]
pub(crate) fn decode_source(bytes: Vec<u8>) -> String {
    match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_bytes_become_replacement_chars_one_per_byte() {
        assert_eq!(
            decode_source(b"<?php // caf\xe9\n".to_vec()),
            "<?php // caf\u{fffd}\n"
        );
        assert_eq!(decode_source("ok é".as_bytes().to_vec()), "ok é");
    }
}
