//! Shared EBML encoding/decoding helpers for Matroska/WebM containers
//!
//! Used by both the MKV demuxer (reading) and MKV muxer (writing).

use std::io::Write;

/// Write an EBML element ID to a byte buffer.
///
/// EBML IDs include their class bits:
/// - 1-byte: `0x80..0xFF`
/// - 2-byte: `0x4000..0x7FFF`
/// - 3-byte: `0x200000..0x3FFFFF`
/// - 4-byte: `0x10000000..0x1FFFFFFF`
pub fn write_id(buf: &mut Vec<u8>, id: u32) {
    if (0x80..=0xFF).contains(&id) {
        buf.push(id as u8);
    } else if (0x4000..=0x7FFF).contains(&id) {
        buf.push((id >> 8) as u8);
        buf.push(id as u8);
    } else if (0x20_0000..=0x3F_FFFF).contains(&id) {
        buf.push((id >> 16) as u8);
        buf.push((id >> 8) as u8);
        buf.push(id as u8);
    } else {
        buf.push((id >> 24) as u8);
        buf.push((id >> 16) as u8);
        buf.push((id >> 8) as u8);
        buf.push(id as u8);
    }
}

/// Write an EBML element ID directly to a writer.
pub fn write_id_to_writer(w: &mut dyn Write, id: u32) -> std::io::Result<()> {
    let mut buf = Vec::new();
    write_id(&mut buf, id);
    w.write_all(&buf)
}

/// Write a variable-length integer (VINT) to a byte buffer.
pub fn write_vint(buf: &mut Vec<u8>, value: u64) {
    if value < 0x7F {
        buf.push(0x80 | value as u8);
    } else if value < 0x3FFF {
        buf.push(0x40 | (value >> 8) as u8);
        buf.push(value as u8);
    } else if value < 0x1F_FFFF {
        buf.push(0x20 | (value >> 16) as u8);
        buf.push((value >> 8) as u8);
        buf.push(value as u8);
    } else {
        buf.push(0x10 | (value >> 24) as u8);
        buf.push((value >> 16) as u8);
        buf.push((value >> 8) as u8);
        buf.push(value as u8);
    }
}

/// Write an EBML unsigned integer element (ID + size + value).
pub fn write_uint(buf: &mut Vec<u8>, id: u32, value: u64) {
    write_id(buf, id);
    if value <= 0xFF {
        write_vint(buf, 1);
        buf.push(value as u8);
    } else if value <= 0xFFFF {
        write_vint(buf, 2);
        buf.push((value >> 8) as u8);
        buf.push(value as u8);
    } else if value <= 0xFFFFFF {
        write_vint(buf, 3);
        buf.push((value >> 16) as u8);
        buf.push((value >> 8) as u8);
        buf.push(value as u8);
    } else {
        write_vint(buf, 4);
        buf.extend_from_slice(&(value as u32).to_be_bytes());
    }
}

/// Write an EBML float element (ID + size + 8-byte f64 value).
pub fn write_float(buf: &mut Vec<u8>, id: u32, value: f64) {
    write_id(buf, id);
    write_vint(buf, 8);
    buf.extend_from_slice(&value.to_be_bytes());
}

/// Write an EBML string element (ID + size + UTF-8 bytes).
pub fn write_string(buf: &mut Vec<u8>, id: u32, value: &str) {
    write_id(buf, id);
    write_vint(buf, value.len() as u64);
    buf.extend_from_slice(value.as_bytes());
}

/// Write an EBML master element (ID + size + child data) to a byte buffer.
pub fn write_master(buf: &mut Vec<u8>, id: u32, data: &[u8]) {
    write_id(buf, id);
    write_vint(buf, data.len() as u64);
    buf.extend_from_slice(data);
}

/// Write an EBML master element directly to a writer.
pub fn write_master_to_writer(w: &mut dyn Write, id: u32, data: &[u8]) -> std::io::Result<()> {
    let mut buf = Vec::new();
    write_master(&mut buf, id, data);
    w.write_all(&buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- write_id tests ---

    #[test]
    fn write_id_1_byte() {
        let mut buf = Vec::new();
        write_id(&mut buf, 0xA3); // SimpleBlock
        assert_eq!(buf, vec![0xA3]);
    }

    #[test]
    fn write_id_1_byte_min() {
        let mut buf = Vec::new();
        write_id(&mut buf, 0x80);
        assert_eq!(buf, vec![0x80]);
    }

    #[test]
    fn write_id_2_byte() {
        let mut buf = Vec::new();
        write_id(&mut buf, 0x4282); // DocType
        assert_eq!(buf, vec![0x42, 0x82]);
    }

    #[test]
    fn write_id_3_byte() {
        let mut buf = Vec::new();
        write_id(&mut buf, 0x2AD7B1); // TimestampScale
        assert_eq!(buf, vec![0x2A, 0xD7, 0xB1]);
    }

    #[test]
    fn write_id_4_byte() {
        let mut buf = Vec::new();
        write_id(&mut buf, 0x1A45DFA3); // EBML header
        assert_eq!(buf, vec![0x1A, 0x45, 0xDF, 0xA3]);
    }

    // --- write_vint tests ---

    #[test]
    fn write_vint_zero() {
        let mut buf = Vec::new();
        write_vint(&mut buf, 0);
        assert_eq!(buf, vec![0x80]); // 1-byte: 0x80 | 0
    }

    #[test]
    fn write_vint_small() {
        let mut buf = Vec::new();
        write_vint(&mut buf, 5);
        assert_eq!(buf, vec![0x85]);
    }

    #[test]
    fn write_vint_max_1_byte() {
        // Max single-byte VINT value is 126 (0x7E); 0x7F is reserved
        let mut buf = Vec::new();
        write_vint(&mut buf, 0x7E);
        assert_eq!(buf, vec![0xFE]);
        assert_eq!(buf.len(), 1);
    }

    #[test]
    fn write_vint_boundary_2_byte() {
        // 0x7F triggers 2-byte encoding
        let mut buf = Vec::new();
        write_vint(&mut buf, 0x7F);
        assert_eq!(buf.len(), 2);
        assert_eq!(buf[0] & 0xC0, 0x40); // 2-byte marker
    }

    #[test]
    fn write_vint_128() {
        let mut buf = Vec::new();
        write_vint(&mut buf, 128);
        assert_eq!(buf.len(), 2);
        assert_eq!(buf[0], 0x40);
        assert_eq!(buf[1], 0x80);
    }

    #[test]
    fn write_vint_16383() {
        // 0x3FFF - max 2-byte value
        let mut buf = Vec::new();
        write_vint(&mut buf, 0x3FFE);
        assert_eq!(buf.len(), 2);
    }

    #[test]
    fn write_vint_3_byte() {
        let mut buf = Vec::new();
        write_vint(&mut buf, 0x4000);
        assert_eq!(buf.len(), 3);
        assert_eq!(buf[0] & 0xE0, 0x20); // 3-byte marker
    }

    #[test]
    fn write_vint_4_byte_large_value() {
        let mut buf = Vec::new();
        write_vint(&mut buf, 0x20_0000);
        assert_eq!(buf.len(), 4);
        assert_eq!(buf[0] & 0xF0, 0x10); // 4-byte marker
    }

    // --- write_uint tests ---

    #[test]
    fn write_uint_small_value() {
        let mut buf = Vec::new();
        write_uint(&mut buf, 0xD7, 1); // TrackNumber = 1
        // ID(1) + size_vint(1) + value(1) = 3 bytes
        assert_eq!(buf.len(), 3);
        assert_eq!(buf[0], 0xD7); // ID
        assert_eq!(buf[1], 0x81); // size = 1 (vint)
        assert_eq!(buf[2], 0x01); // value
    }

    #[test]
    fn write_uint_two_byte_value() {
        let mut buf = Vec::new();
        write_uint(&mut buf, 0xD7, 0x0100);
        // ID(1) + size_vint(1) + value(2) = 4 bytes
        assert_eq!(buf.len(), 4);
        assert_eq!(buf[2], 0x01);
        assert_eq!(buf[3], 0x00);
    }

    #[test]
    fn write_uint_three_byte_value() {
        let mut buf = Vec::new();
        write_uint(&mut buf, 0xD7, 0x01_0000);
        // ID(1) + size_vint(1) + value(3) = 5 bytes
        assert_eq!(buf.len(), 5);
    }

    #[test]
    fn write_uint_four_byte_value() {
        let mut buf = Vec::new();
        write_uint(&mut buf, 0xD7, 0x0100_0000);
        // ID(1) + size_vint(1) + value(4) = 6 bytes
        assert_eq!(buf.len(), 6);
    }

    #[test]
    fn write_uint_roundtrip_value_preserved() {
        let mut buf = Vec::new();
        let value: u64 = 48000;
        write_uint(&mut buf, 0xB5, value); // SamplingFrequency-like
        // The value bytes are the last 2 bytes (48000 fits in 2 bytes)
        let val_bytes = &buf[buf.len() - 2..];
        let decoded = (val_bytes[0] as u64) << 8 | val_bytes[1] as u64;
        assert_eq!(decoded, value);
    }

    // --- write_float tests ---

    #[test]
    fn write_float_element() {
        let mut buf = Vec::new();
        write_float(&mut buf, 0xB5, 48000.0);
        // ID(1-byte 0xB5) + size_vint(1, value=8) + f64(8) = 10 bytes
        assert_eq!(buf.len(), 10);
        // Verify the f64 bytes at the end
        let float_bytes: [u8; 8] = buf[2..10].try_into().unwrap();
        let decoded = f64::from_be_bytes(float_bytes);
        assert_eq!(decoded, 48000.0);
    }

    #[test]
    fn write_float_zero() {
        let mut buf = Vec::new();
        write_float(&mut buf, 0x80, 0.0);
        let float_bytes: [u8; 8] = buf[2..10].try_into().unwrap();
        assert_eq!(f64::from_be_bytes(float_bytes), 0.0);
    }

    // --- write_string tests ---

    #[test]
    fn write_string_element() {
        let mut buf = Vec::new();
        write_string(&mut buf, 0x4282, "matroska");
        // ID(2) + size_vint(1) + "matroska"(8) = 11 bytes
        assert_eq!(buf.len(), 11);
        assert_eq!(&buf[3..], b"matroska");
    }

    #[test]
    fn write_string_empty() {
        let mut buf = Vec::new();
        write_string(&mut buf, 0x80, "");
        // ID(1) + size_vint(1, value=0) = 2 bytes, no data
        assert_eq!(buf.len(), 2);
    }

    #[test]
    fn write_string_preserves_utf8() {
        let mut buf = Vec::new();
        let text = "hello\u{00e9}"; // 7 UTF-8 bytes
        write_string(&mut buf, 0x80, text);
        let str_bytes = &buf[2..];
        assert_eq!(str_bytes, text.as_bytes());
    }

    // --- write_master tests ---

    #[test]
    fn write_master_wraps_inner() {
        let mut inner = Vec::new();
        write_uint(&mut inner, 0xD7, 1);
        write_string(&mut inner, 0x86, "V_VP9");
        let inner_len = inner.len();

        let mut buf = Vec::new();
        write_master(&mut buf, 0xAE, &inner); // TrackEntry
        // ID(1) + size_vint(1) + inner
        assert_eq!(buf.len(), 2 + inner_len);
        // The tail should be identical to inner
        assert_eq!(&buf[2..], &inner);
    }

    #[test]
    fn write_master_empty() {
        let mut buf = Vec::new();
        write_master(&mut buf, 0xAE, &[]);
        // ID(1) + size_vint(1, value=0) = 2 bytes
        assert_eq!(buf.len(), 2);
    }

    #[test]
    fn write_master_nested() {
        // Build an inner master, then wrap it in an outer master
        let mut inner = Vec::new();
        write_uint(&mut inner, 0xD7, 1);

        let mut mid = Vec::new();
        write_master(&mut mid, 0xAE, &inner);

        let mut outer = Vec::new();
        write_master(&mut outer, 0x1654AE6B, &mid); // Tracks
        // Outer ID is 4-byte
        assert_eq!(outer[..4], [0x16, 0x54, 0xAE, 0x6B]);
        // Tail should match mid
        let size_byte = 1; // mid.len() is small
        assert_eq!(&outer[4 + size_byte..], &mid);
    }
}
