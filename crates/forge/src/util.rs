use crate::error::{Error, Result};

pub(crate) fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (&x, &y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

pub(crate) fn random_hex_token<const BYTES: usize>() -> Result<String> {
    let mut buf = [0u8; BYTES];
    getrandom::fill(&mut buf)
        .map_err(|e| Error::Runtime(format!("failed to generate cryptographic token: {e}")))?;
    let mut out = String::with_capacity(BYTES * 2);
    for b in buf {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{b:02x}");
    }
    Ok(out)
}

pub(crate) fn sanitize_identifier(identifier: &str) -> String {
    identifier
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}
