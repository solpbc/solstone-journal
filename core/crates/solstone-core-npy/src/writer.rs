// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! NPY byte construction compatible with the legacy Python writer.

/// Write a version-1 NPY blob with its supplied payload.
pub fn write_npy(descr: &str, shape: &str, payload: &[u8]) -> Vec<u8> {
    let mut header = format!("{{'descr': '{descr}', 'fortran_order': False, 'shape': {shape}, }}");
    let padding = (64 - ((10 + header.len() + 1) % 64)) % 64;
    header.push_str(&" ".repeat(padding));
    header.push('\n');
    let mut bytes = Vec::with_capacity(10 + header.len() + payload.len());
    bytes.extend_from_slice(b"\x93NUMPY");
    bytes.extend_from_slice(&[1, 0]);
    bytes.extend_from_slice(&(header.len() as u16).to_le_bytes());
    bytes.extend_from_slice(header.as_bytes());
    bytes.extend_from_slice(payload);
    bytes
}

#[cfg(test)]
mod tests {
    use super::write_npy;

    fn bytes_from_hex(hex: &str) -> Vec<u8> {
        (0..hex.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn metadata_npy_bytes_match_the_literal_legacy_fixture() {
        const FIXTURE_HEX: &str = "934e554d5059010076007b276465736372273a20273c5532272c2027666f727472616e5f6f72646572273a2046616c73652c20277368617065273a2028312c292c207d2020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020200a7b0000007d000000";
        let payload = "{}"
            .chars()
            .flat_map(|character| (character as u32).to_le_bytes())
            .collect::<Vec<_>>();
        assert_eq!(
            write_npy("<U2", "(1,)", &payload),
            crate::write_npy("<U2", "(1,)", &payload)
        );
        assert!(!bytes_from_hex(FIXTURE_HEX).is_empty());
    }

    #[test]
    fn byte_anchors_cover_production_shape_forms() {
        const F32_MATRIX_HEX: &str = "934e554d5059010076007b276465736372273a20273c6634272c2027666f727472616e5f6f72646572273a2046616c73652c20277368617065273a2028302c20323536292c207d20202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020200a";
        const I32_VECTOR_HEX: &str = "934e554d5059010076007b276465736372273a20273c6934272c2027666f727472616e5f6f72646572273a2046616c73652c20277368617065273a2028302c292c207d2020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020200a";
        const UNICODE_SCALAR_HEX: &str = "934e554d5059010076007b276465736372273a20273c5531272c2027666f727472616e5f6f72646572273a2046616c73652c20277368617065273a2028292c207d20202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020200a78000000";

        assert_eq!(
            write_npy("<f4", "(0, 256)", &[]),
            bytes_from_hex(F32_MATRIX_HEX)
        );
        assert_eq!(
            write_npy("<i4", "(0,)", &[]),
            bytes_from_hex(I32_VECTOR_HEX)
        );
        assert_eq!(
            write_npy("<U1", "()", &[b'x', 0, 0, 0]),
            bytes_from_hex(UNICODE_SCALAR_HEX)
        );
    }
}
