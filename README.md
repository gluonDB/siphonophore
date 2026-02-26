# Siphonophore

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
[doc_id_len: u8][doc_id: bytes][yjs_payload: bytes]
```

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

## Client Example (JavaScript)

```javascript
import * as Y from 'yjs'

const doc = new Y.Doc()
const ws = new WebSocket('ws://localhost:8080/ws')

ws.binaryType = 'arraybuffer'

ws.onmessage = (event) => {
  const data = new Uint8Array(event.data)
  const docIdLen = data[0]
  const docId = new TextDecoder().decode(data.slice(1, 1 + docIdLen))
  const payload = data.slice(1 + docIdLen)
  // Handle Yjs sync message...
}

function send(docId, payload) {
  const encoder = new Uint8Array(1 + docId.length + payload.length)
  encoder[0] = docId.length
  encoder.set(new TextEncoder().encode(docId), 1)
  encoder.set(payload, 1 + docId.length)
  ws.send(encoder)
}
```

## License

MIT OR Apache-2.0
