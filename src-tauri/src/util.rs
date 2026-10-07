//! 通用小工具：十六进制编解码（快照里逐字节保留 hosts 原文用）

pub fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0x0f) as usize] as char);
    }
    s
}

pub fn decode_hex(hex: &str) -> Option<Vec<u8>> {
    let h = hex.trim();
    if h.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(h.len() / 2);
    let bytes = h.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let hi = (bytes[i] as char).to_digit(16)?;
        let lo = (bytes[i + 1] as char).to_digit(16)?;
        out.push((hi * 16 + lo) as u8);
        i += 2;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip() {
        let data = vec![0u8, 1, 15, 16, 255, 128];
        let hex = encode_hex(&data);
        assert_eq!(hex, "00010f10ff80");
        assert_eq!(decode_hex(&hex).unwrap(), data);
        assert!(decode_hex("abc").is_none());
        assert!(decode_hex("zz").is_none());
    }
}
