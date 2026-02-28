const encoder = new TextEncoder();
const decoder = new TextDecoder();

export function encodeWire(docId: string, payload: Uint8Array): Uint8Array {
  const idBytes = encoder.encode(docId);
  const msg = new Uint8Array(1 + idBytes.length + payload.length);
  msg[0] = idBytes.length;
  msg.set(idBytes, 1);
  msg.set(payload, 1 + idBytes.length);
  return msg;
}

export function decodeWire(data: Uint8Array): { docId: string; payload: Uint8Array } {
  const idLen = data[0];
  const docId = decoder.decode(data.slice(1, 1 + idLen));
  const payload = data.slice(1 + idLen);
  return { docId, payload };
}
