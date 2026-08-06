# Siphonophore


[![sloprank](https://sloprank.io/badge/gluondb/siphonophore.svg)](https://sloprank.io/repo/gluondb/siphonophore)

Extensible Yjs sync server primitive.

This aims to be an alternative to [Hocuspocus](https://github.com/ueberdosis/hocuspocus), but with native document multiplexing in a single WebSocket connection. 

## Features

- **Multiplexing**: Multiple documents over a single WebSocket connection
- **Hooks**: Extend behavior without your application logic.
- **Actor Model**: Built with the awesome libraries of [Kameo](https://github.com/tqwewe/kameo) and [Yrs](https://github.com/y-crdt/y-crdt)
- **Axum Integration**: Compose with your existing HTTP routes
- **Dead simple multiplexing protocol**: Can be used in any YJS compatible client, with minimal effort.


## Projects Using Siphonophore

<a href="https://gluondb.com"><img src="https://avatars.githubusercontent.com/u/235569532?v=4" width="40"></a> &nbsp; <a href="https://diaryx.org"><img src="https://avatars.githubusercontent.com/u/243425743?v=4" width="40"></a>


## Quick Start

```rust
use siphonophore::Server;

#[tokio::main]
async fn main() {
    Server::new()
        .serve("0.0.0.0:8080")
        .await
        .unwrap();
}
```

## With Persistence

```rust
use siphonophore::{Server, Hook, HookResult, OnLoadDocumentPayload, BeforeCloseDirtyPayload};
use async_trait::async_trait;

struct FileStorage;

#[async_trait]
impl Hook for FileStorage {
    async fn on_load_document(&self, p: OnLoadDocumentPayload<'_>) 
        -> Result<Option<Vec<u8>>, Box<dyn std::error::Error + Send + Sync>> 
    {
        let path = format!("data/{}.bin", p.doc_id);
        match std::fs::read(&path) {
            Ok(data) => Ok(Some(data)),
            Err(_) => Ok(None),
        }
    }

    async fn before_close_dirty(&self, p: BeforeCloseDirtyPayload<'_>) -> HookResult {
        let path = format!("data/{}.bin", p.doc_id);
        std::fs::write(&path, p.state)?;
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    Server::with_hooks(vec![Box::new(FileStorage)])
        .serve("0.0.0.0:8080")
        .await
        .unwrap();
}
```

## Authentication

Use the `on_authenticate` hook to validate and store user info in the context for later hooks.

```rust
use siphonophore::{Server, Hook, HookResult, OnAuthenticatePayload, OnChangePayload};
use async_trait::async_trait;

// Your user type
#[derive(Clone)]
struct User {
    id: String,
    name: String,
}

struct AuthHook {
    // Your auth service, DB pool, etc.
}

#[async_trait]
impl Hook for AuthHook {
    async fn on_authenticate(&self, p: OnAuthenticatePayload<'_>) -> HookResult {
        // Token is auto-extracted from ?token= or Authorization: Bearer
        let token = p.request.token.as_ref()
            .ok_or("No token provided")?;
        
        // Validate token (call your auth service, verify JWT, etc.)
        let user = validate_token(token).await
            .map_err(|_| "Invalid token")?;
        if !user_can_access(&user, p.doc_id) {
            return Err("Access denied".into());
        }
        p.context.insert(user);
        Ok(())
    }
    
    async fn on_change(&self, p: OnChangePayload<'_>) -> HookResult {
        // Access the user from context
        if let Some(user) = p.context.get::<User>() {
            println!("{} edited {}", user.name, p.doc_id);
        }
        Ok(())
    }
}

async fn validate_token(token: &str) -> Result<User, ()> {
    Ok(User { id: "123".into(), name: "Alice".into() })
}
fn user_can_access(user: &User, doc_id: &str) -> bool {
    true
}

#[tokio::main]
async fn main() {
    Server::with_hooks(vec![Box::new(AuthHook {})])
        .serve("0.0.0.0:8080")
        .await
        .unwrap();
}
```

**Client-side:**

```javascript
// Via query param
const ws = new WebSocket('ws://localhost:8080/ws?token=your-jwt-token')

// Or via header (if your client supports it)
const ws = new WebSocket('ws://localhost:8080/ws', {
  headers: { 'Authorization': 'Bearer your-jwt-token' }
})
```

## Custom WebSocket Path

```rust
use siphonophore::Server;

#[tokio::main]
async fn main() {
    // Mount at custom path instead of default /ws
    let app = Server::new().into_router_at("/sync/websocket");
    
    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
```

## Composing with Axum

```rust
use siphonophore::Server;
use axum::{Router, routing::get};

#[tokio::main]
async fn main() {
    let server = Server::new();
    let handle = server.handle();

    let app = Router::new()
        .merge(server.into_router_at("/collab"))  // Custom path
        .route("/health", get(|| async { "ok" }))
        .route("/save/:doc", get(move |path: axum::extract::Path<String>| {
            let h = handle.clone();
            async move { h.persist_document(&path).await; "saved" }
        }));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
```

## Wire Protocol

Messages are prefixed with doc_id for multiplexing:

```
[doc_id_len: u8][doc_id: bytes][payload: bytes]
```

Each document must complete incarnation negotiation before sending Yjs or
awareness payloads. Incarnation UUIDs exist only for the lifetime of the
in-memory document actor.

| Payload | Meaning |
|---------|---------|
| `[2, 0]` or `[2, 0, previous_uuid...]` | Join without sharing CRDT state |
| `[2, 1, current_uuid...]` | Current server incarnation |
| `[2, 2, current_uuid...]` | Client is ready to begin Yjs sync |
| `[2, 3, error_code]` | Protocol error |

The server rejects Yjs (`0`) and awareness (`1`) messages until a matching
`Ready` is received. This prevents a retained client document from merging
with an independently re-created server document.

Note that message type `2` is `messageAuth` in y-protocols. Siphonophore
reclaims it for incarnation negotiation and never forwards it to the Yjs
protocol handler, so clients must not send y-protocols auth messages on this
transport.

| Error code | Meaning |
|------------|---------|
| `1` | Malformed incarnation message |
| `2` | Payload sent before `Ready` was accepted |
| `3` | `Ready` did not match the offered incarnation |
| `4` | An `on_connect` or `on_authenticate` hook rejected the client |
| `5` | The document failed to load |
| `6` | The 30s join window expired |
| `7` | The document was unloaded by the host application |

## Control Messages (Text WebSocket)

```json
{"action": "leave", "doc": "my-document"}
{"action": "save", "doc": "my-document"}
```

## Hooks

| Hook | When | Use Case |
|------|------|----------|
| `on_connect` | Client tries to access doc | Rate limiting, logging |
| `on_authenticate` | After connect | Auth, set user context |
| `on_load_document` | Doc first loaded | Load from storage |
| `on_change` | Every update | Real-time webhooks |
| `on_disconnect` | Client leaves doc | Analytics |
| `on_save` | Explicit save request | Checkpoints |
| `before_close_dirty` | Before unloading dirty doc | Lazypersistence |
| `after_unload_document` | Doc fully unloaded | Cache invalidation |

## JavaScript Client

Install [`@siphonophore/client`](https://www.npmjs.com/package/@siphonophore/client):

```bash
npm install @siphonophore/client
```

```javascript
import { SiphonophoreSocket } from '@siphonophore/client'

const socket = new SiphonophoreSocket('ws://localhost:8080/ws')
let incarnation

socket.onconnect(() => {
  socket.join('my-doc', incarnation)
})

socket.onincarnation('my-doc', (current) => {
  if (incarnation && incarnation !== current) {
    // Reconcile or replace the application-owned Y.Doc here. Never send the
    // old Y.Doc to a different incarnation.
  }
  incarnation = current
  socket.ready('my-doc', current)
  // Start the normal y-protocols sync exchange now.
})
```

The client intentionally has no Yjs dependency; applications layer
`y-protocols` and their own incarnation reconciliation policy on top.

## License

MIT OR Apache-2.0
