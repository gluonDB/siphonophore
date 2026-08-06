# @siphonophore/client

JavaScript client for [Siphonophore](https://github.com/gluonDB/siphonophore) — a multiplexed Yjs sync server. Handles the binary wire protocol, incarnation negotiation, and reconnection with exponential backoff. Zero Yjs dependencies; bring your own y-protocols on top.

## Install

```bash
npm install @siphonophore/client
```

## Usage

Every document must complete incarnation negotiation before the server will
accept sync traffic for it. An incarnation is a UUID identifying one in-memory
lifetime of a server-side document: call `join`, wait for `onincarnation`, then
call `ready` to unlock sync.

```javascript
import * as Y from 'yjs'
import * as syncProtocol from 'y-protocols/sync'
import * as encoding from 'lib0/encoding'
import * as decoding from 'lib0/decoding'
import { SiphonophoreSocket, ProtocolErrorCode } from '@siphonophore/client'

const MSG_SYNC = 0
const socket = new SiphonophoreSocket('ws://localhost:8080/ws')
let doc = new Y.Doc()
let incarnation

// Re-join on every (re)connect; the server forgets clients when a socket drops
socket.onconnect(() => {
  socket.join('my-doc', incarnation)
})

socket.onincarnation('my-doc', (current) => {
  if (incarnation && incarnation !== current) {
    // The server document was re-created. Anything in the old Y.Doc predates
    // this incarnation, so replace it rather than merging it in.
    doc = new Y.Doc()
  }
  incarnation = current
  // Unlocks sync; the server immediately follows with its SyncStep1
  socket.ready('my-doc', current)
})

// Payloads are y-protocols messages, not bare Yjs updates
socket.subscribe('my-doc', (payload) => {
  const decoder = decoding.createDecoder(payload)
  if (decoding.readVarUint(decoder) !== MSG_SYNC) return
  const encoder = encoding.createEncoder()
  encoding.writeVarUint(encoder, MSG_SYNC)
  syncProtocol.readSyncMessage(decoder, encoder, doc, null)
  if (encoding.length(encoder) > 1) {
    socket.send('my-doc', encoding.toUint8Array(encoder))
  }
})
```

Anything sent before `ready` is accepted gets dropped with
`ProtocolErrorCode.NotReady`. Handle the other failures via `onprotocolerror`:

```javascript
socket.onprotocolerror('my-doc', (code) => {
  if (
    code === ProtocolErrorCode.DocumentUnloaded ||
    code === ProtocolErrorCode.JoinExpired ||
    code === ProtocolErrorCode.StaleIncarnation
  ) {
    incarnation = undefined
    socket.join('my-doc')
  }
})
```

Multiple documents share the same connection, each negotiating independently:

```javascript
for (const id of ['doc-a', 'doc-b']) {
  socket.onincarnation(id, (current) => socket.ready(id, current))
  socket.subscribe(id, (payload) => handleSync(id, payload))
  socket.join(id)
}
```

## API

### `new SiphonophoreSocket(url, opts?)`

| Option | Default | Description |
|---|---|---|
| `reconnectBaseMs` | `500` | Base delay for exponential backoff |
| `reconnectMaxMs` | `30000` | Maximum reconnect delay |

### Methods

| Method | Description |
|---|---|
| `join(docId, previousIncarnation?)` | Begin negotiation for a doc. Sends no CRDT state. |
| `ready(docId, incarnation)` | Confirm the incarnation and unlock sync for the doc. |
| `onincarnation(docId, handler)` | Receive the server's current incarnation. Returns unsubscribe fn. |
| `onprotocolerror(docId, handler)` | Receive `ProtocolErrorCode` values for a doc. Returns unsubscribe fn. |
| `subscribe(docId, handler)` | Receive raw Yjs/awareness payloads for a doc. Returns unsubscribe fn. |
| `send(docId, payload)` | Send a raw `Uint8Array` payload for a doc. |
| `onconnect(fn)` | Called every time the socket (re)opens. Returns unsubscribe fn. |
| `ondisconnect(fn)` | Called every time the socket closes. Returns unsubscribe fn. |
| `save(docId)` | Ask the server to persist a doc's current state. |
| `leave(docId)` | Tell the server this client is leaving a doc. |
| `destroy()` | Close the socket and stop reconnecting. |

### `ProtocolErrorCode`

| Value | Code | Meaning |
|---|---|---|
| `InvalidMessage` | 1 | Malformed incarnation message |
| `NotReady` | 2 | Payload sent before `ready` was accepted |
| `StaleIncarnation` | 3 | `ready` did not match the offered incarnation |
| `Authentication` | 4 | A server hook rejected the client |
| `LoadFailed` | 5 | The document failed to load |
| `JoinExpired` | 6 | The 30s join window expired; call `join` again |
| `DocumentUnloaded` | 7 | The host unloaded the doc; re-join for a new incarnation |

### `socket.connected`

`true` if the underlying WebSocket is currently open.

## Wire Protocol

Messages are framed as: `[doc_id_len: u8][doc_id: bytes][payload: bytes]`

The `encodeWire` / `decodeWire` helpers are also exported if you need direct access.

## License

MIT OR Apache-2.0
