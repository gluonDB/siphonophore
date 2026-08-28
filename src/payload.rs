use bytes::Bytes;

/// Encode payload with doc_id prefix for multiplexing.
///
/// Wire format: `[u16 LE: doc_id_len][doc_id bytes][payload bytes]`
pub fn encode_with_doc_id(doc_id: &str, payload: &[u8]) -> Bytes {
    let id = doc_id.as_bytes();
    let len = id.len() as u16;
    let mut buf = Vec::with_capacity(2 + id.len() + payload.len());
    buf.extend_from_slice(&len.to_le_bytes());
    buf.extend_from_slice(id);
    buf.extend_from_slice(payload);
    buf.into()
}

/// Decode doc_id and payload from wire format.
///
/// Wire format: `[u16 LE: doc_id_len][doc_id bytes][payload bytes]`
pub fn decode_doc_id(data: &[u8]) -> Option<(&str, &[u8])> {
    if data.len() < 2 { return None; }
    let len = u16::from_le_bytes([data[0], data[1]]) as usize;
    if data.len() < 2 + len { return None; }
    let doc_id = std::str::from_utf8(&data[2..2 + len]).ok()?;
    Some((doc_id, &data[2 + len..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let doc_id = "file:ns-uuid-123/notes/hello.md";
        let payload = b"some yrs update bytes";
        let encoded = encode_with_doc_id(doc_id, payload);
        let (decoded_id, decoded_payload) = decode_doc_id(&encoded).unwrap();
        assert_eq!(decoded_id, doc_id);
        assert_eq!(decoded_payload, payload);
    }

    #[test]
    fn empty_payload() {
        let encoded = encode_with_doc_id("manifest:abc", b"");
        let (id, payload) = decode_doc_id(&encoded).unwrap();
        assert_eq!(id, "manifest:abc");
        assert!(payload.is_empty());
    }

    #[test]
    fn too_short() {
        assert!(decode_doc_id(&[]).is_none());
        assert!(decode_doc_id(&[1]).is_none());
        assert!(decode_doc_id(&[5, 0]).is_none()); // claims 5 bytes but none follow
    }

    #[test]
    fn long_doc_id() {
        let doc_id = "file:".to_string() + &"a".repeat(500);
        let payload = b"data";
        let encoded = encode_with_doc_id(&doc_id, payload);
        let (decoded_id, decoded_payload) = decode_doc_id(&encoded).unwrap();
        assert_eq!(decoded_id, doc_id);
        assert_eq!(decoded_payload, payload);
    }
}
