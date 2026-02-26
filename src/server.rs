use axum::{Router, routing::get, extract::{ws::WebSocketUpgrade, State, Query}, response::Response, http::HeaderMap};
use kameo::actor::{ActorRef, Spawn};
use std::io;
use std::collections::HashMap;
use std::sync::Arc;

use crate::actor::{Root, CreateClient, PersistDocument, ApplyServerUpdate, BroadcastText, GetDocPeerCount};
use crate::hooks::{Hook, RequestInfo};
use kameo::actor::ActorId;

/// Siphonophore sync server
/// 
/// # Flexible mounting
/// ```no_run
/// use siphonophore::Server;
/// use axum::Router;
/// 
/// // Option 1: Default router with /ws
/// let app = Server::new().into_router();
/// 
/// // Option 2: Custom path
/// let app = Server::new().into_router_at("/sync/ws");
/// 
/// // Option 3: Compose with other routes
/// let server = Server::new();
/// let handle = server.handle();
/// let app = Router::new()
///     .merge(server.into_router_at("/collab"))
///     .route("/api/save/:doc", axum::routing::post(move |path: axum::extract::Path<String>| {
///         let h = handle.clone();
///         async move { h.persist_document(&path).await; "ok" }
///     }));
/// ```
#[derive(Clone)]
pub struct Server {
    root: ActorRef<Root>,
}

/// Handle for interacting with the server from HTTP handlers
#[derive(Clone)]
pub struct Handle {
    root: ActorRef<Root>,
}

impl Handle {
    /// Force persist a document's current state
    pub async fn persist_document(&self, doc_id: &str) {
        let doc_id: Arc<str> = doc_id.into();
        let _ = self.root.ask(PersistDocument(doc_id)).send().await;
    }

    /// Apply a Yjs update to a document if it's loaded. Returns true if applied.
    pub async fn apply_update(&self, doc_id: &str, update: Vec<u8>) -> bool {
        let doc_id: Arc<str> = doc_id.into();
        self.root.ask(ApplyServerUpdate { doc_id, update }).send().await.unwrap_or(false)
    }

    /// Broadcast a text message to all clients connected to a document.
    ///
    /// Use this for sending control messages like peer events, focus changes, etc.
    ///
    /// # Arguments
    /// * `doc_id` - The document ID
    /// * `message` - The text message to broadcast (usually JSON)
    /// * `exclude_client` - Optional client to exclude from the broadcast
    pub async fn broadcast_text(&self, doc_id: &str, message: String, exclude_client: Option<ActorId>) -> bool {
        let doc_id: Arc<str> = doc_id.into();
        self.root.ask(BroadcastText { doc_id, message, exclude_client }).send().await.unwrap_or(false)
    }

    /// Get the number of peers connected to a document.
    pub async fn get_peer_count(&self, doc_id: &str) -> usize {
        let doc_id: Arc<str> = doc_id.into();
        self.root.ask(GetDocPeerCount(doc_id)).send().await.unwrap_or(0)
    }
}

impl Server {
    pub fn new() -> Self {
        let root = Root::spawn(Root::new());
        Self { root }
    }

    pub fn with_hooks(hooks: Vec<Box<dyn Hook>>) -> Self {
        let root = Root::spawn(Root::with_hooks(hooks));
        Self { root }
    }

    pub async fn persist_document(&self, doc_id: &str) {
        let doc_id: Arc<str> = doc_id.into();
        let _ = self.root.ask(PersistDocument(doc_id)).send().await;
    }

    /// Get a handle for use in other HTTP handlers
    pub fn handle(&self) -> Handle {
        Handle { root: self.root.clone() }
    }

    /// Get router with WebSocket endpoint at `/ws`
    pub fn into_router(self) -> Router {
        self.into_router_at("/ws")
    }

    /// Get router with WebSocket endpoint at a custom path
    pub fn into_router_at(self, path: &str) -> Router {
        Router::new()
            .route(path, get(ws_handler))
            .with_state(self.root)
    }

    /// Start the server on the given address with default `/ws` path
    pub async fn serve(self, addr: &str) -> io::Result<()> {
        let app = self.into_router();
        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, app).await
    }
}

impl Default for Server { fn default() -> Self { Self::new() } }

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(root): State<ActorRef<Root>>,
    headers: HeaderMap,
    Query(query_params): Query<HashMap<String, String>>,
) -> Response {
    let headers_map: HashMap<String, String> = headers
        .iter()
        .filter_map(|(k, v)| v.to_str().ok().map(|v| (k.as_str().to_lowercase(), v.to_string())))
        .collect();
    let request_info = RequestInfo::new(headers_map, query_params);
    
    ws.on_upgrade(move |socket| async move {
        let _ = root.ask(CreateClient { socket, request_info }).send().await;
    })
}
