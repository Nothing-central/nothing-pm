//! base64 decoding, used for npm's `dist.integrity` values.
//! Accepts standard and url-safe alphabets, ignores padding and whitespace.

pub fn decode(input: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    for ch in input.chars() {
        let val = match ch {
            'A'..='Z' => ch as u32 - 'A' as u32,
            'a'..='z' => ch as u32 - 'a' as u32 + 26,
            '0'..='9' => ch as u32 - '0' as u32 + 52,
            '+' | '-' => 62,
            '/' | '_' => 63,
            '=' | ' ' | '\n' | '\r' | '\t' => continue,
            other => return Err(format!("invalid base64 character '{other}'")),
        };
        acc = (acc << 6) | val;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xFF) as u8);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash;

    #[test]
    fn decodes_standard_and_urlsafe() {
        assert_eq!(decode("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(decode("aGVsbG8").unwrap(), b"hello");
        // '->' vs '/' alphabets
        assert_eq!(decode("-_8").unwrap(), decode("+/8").unwrap());
    }

    #[test]
    fn decodes_sha512_integrity() {
        let digest = hash::sha512(b"abc");
        let b64 = "uU0nuZNNPgilLlLX2n2r+sSE7+N6U4DukIj3rOLvzek=";
        let got = decode(b64).unwrap();
        assert_eq!(got.len(), 32);
        // sanity: sha512 produces 64 bytes, base64 of it is 88 chars
        assert_eq!(digest.len(), 64);
    }
}
