use bytes::Bytes;

/// Encode payload with doc_id prefix for multiplexing
pub fn encode_with_doc_id(doc_id: &str, payload: &[u8]) -> Bytes {
    let id = doc_id.as_bytes();
    let len = id.len().min(255) as u8;
    let mut buf = Vec::with_capacity(1 + len as usize + payload.len());
    buf.push(len);
    buf.extend_from_slice(&id[..len as usize]);
    buf.extend_from_slice(payload);
    buf.into()
}

/// Decode doc_id and payload from wire format
pub fn decode_doc_id(data: &[u8]) -> Option<(&str, &[u8])> {
    let len = *data.first()? as usize;
    if data.len() < 1 + len {
        return None;
    }
    let doc_id = std::str::from_utf8(&data[1..1 + len]).ok()?;
    Some((doc_id, &data[1 + len..]))
}
