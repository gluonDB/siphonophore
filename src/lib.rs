//! # Siphonophore
//!
//! Extensible Yjs sync server primitive in ~850 lines of code.
//!
//! ## Quick Start
//!
//! ```no_run
//! use siphonophore::Server;
//!
//! #[tokio::main]
//! async fn main() {
//!     Server::new()
//!         .serve("0.0.0.0:8080")
//!         .await
//!         .unwrap();
//! }
//! ```
//!
//! ## Custom Path
//!
//! ```no_run
//! use siphonophore::Server;
//!
//! #[tokio::main]
//! async fn main() {
//!     let app = Server::new().into_router_at("/sync/ws");
//!     let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await.unwrap();
//!     axum::serve(listener, app).await.unwrap();
//! }
//! ```
//!
//! ## With Persistence Hook
//!
//! ```no_run
//! use siphonophore::{Server, Hook, HookResult, OnLoadDocumentPayload, BeforeCloseDirtyPayload};
//! use async_trait::async_trait;
//!
//! struct MyStorage;
//!
//! #[async_trait]
//! impl Hook for MyStorage {
//!     async fn on_load_document(&self, p: OnLoadDocumentPayload<'_>) -> Result<Option<Vec<u8>>, Box<dyn std::error::Error + Send + Sync>> {
//!         Ok(None) // Load from your storage
//!     }
//!
//!     async fn before_close_dirty(&self, p: BeforeCloseDirtyPayload<'_>) -> HookResult {
//!         println!("Saving {} ({} bytes)", p.doc_id, p.state.len());
//!         Ok(()) // Save p.state to your storage
//!     }
//! }
//!
//! #[tokio::main]
//! async fn main() {
//!     Server::with_hooks(vec![Box::new(MyStorage)])
//!         .serve("0.0.0.0:8080")
//!         .await
//!         .unwrap();
//! }
//! ```
//!
//! ## Composing with Axum
//!
//! ```no_run
//! use siphonophore::Server;
//! use axum::{Router, routing::get};
//!
//! #[tokio::main]
//! async fn main() {
//!     let server = Server::new();
//!     let handle = server.handle();
//!
//!     let app = Router::new()
//!         .merge(server.into_router_at("/collab"))
//!         .route("/health", get(|| async { "ok" }));
//!
//!     let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await.unwrap();
//!     axum::serve(listener, app).await.unwrap();
//! }
//! ```

mod hooks;
mod actor;
mod payload;
mod server;

// Public API
pub use server::{Server, Handle};
pub use hooks::{
    Hook, HookResult, HookError, Context, RequestInfo,
    OnConnectPayload, OnAuthenticatePayload, OnLoadDocumentPayload,
    OnChangePayload, OnDisconnectPayload, OnSavePayload, BeforeCloseDirtyPayload,
    OnBeforeSyncPayload, BeforeSyncAction,
    OnControlMessagePayload, ControlMessageResponse,
    OnPeerJoinedPayload, OnPeerLeftPayload,
};

pub use axum;
pub use async_trait::async_trait;
pub use kameo::actor::ActorId;
