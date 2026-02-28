use async_trait::async_trait;
use kameo::actor::ActorId;
use yrs::Doc;
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::error::Error;
use std::sync::Arc;

pub type HookResult = Result<(), Box<dyn Error + Send + Sync>>;

/// Connection-scoped state shared between hooks.
#[derive(Default, Clone)]
pub struct Context(HashMap<TypeId, Arc<dyn Any + Send + Sync>>);

impl Context {
    pub fn insert<T: Send + Sync + 'static>(&mut self, val: T) {
        self.0.insert(TypeId::of::<T>(), Arc::new(val));
    }

    pub fn get<T: 'static>(&self) -> Option<&T> {
        self.0.get(&TypeId::of::<T>()).and_then(|arc| arc.downcast_ref())
    }
}

/// HTTP request info captured at WebSocket upgrade.
#[derive(Clone, Default)]
pub struct RequestInfo {
    pub headers: HashMap<String, String>,
    pub query_params: HashMap<String, String>,
    pub token: Option<String>,
}

impl RequestInfo {
    pub fn new(headers: HashMap<String, String>, query_params: HashMap<String, String>) -> Self {
        let token = query_params.get("token").cloned().or_else(|| {
            headers.get("authorization").and_then(|h| h.strip_prefix("Bearer ").map(|t| t.to_string()))
        });
        Self { headers, query_params, token }
    }
}

// ============================================================================
// Payloads
// ============================================================================

pub struct OnConnectPayload<'a> {
    pub doc_id: &'a str,
    pub client_id: ActorId,
    pub request: &'a RequestInfo,
}

pub struct OnAuthenticatePayload<'a> {
    pub doc_id: &'a str,
    pub client_id: ActorId,
    pub request: &'a RequestInfo,
    pub context: &'a mut Context,
}

pub struct OnLoadDocumentPayload<'a> {
    pub doc_id: &'a str,
}

pub struct OnChangePayload<'a> {
    pub doc_id: &'a str,
    pub client_id: ActorId,
    pub update: &'a [u8],
    pub doc: &'a Doc,
    pub context: &'a Context,
}

pub struct OnDisconnectPayload<'a> {
    pub doc_id: &'a str,
    pub client_id: ActorId,
    pub context: &'a Context,
}

pub struct OnSavePayload<'a> {
    pub doc_id: &'a str,
    pub state: &'a [u8],
    pub doc: &'a Doc,
}

pub struct BeforeCloseDirtyPayload<'a> {
    pub doc_id: &'a str,
    pub state: &'a [u8],
    pub doc: &'a Doc,
}

// ============================================================================
// Hook Trait
// ============================================================================

#[async_trait]
pub trait Hook: Send + Sync {
    /// Called when a client first tries to access a document.
    async fn on_connect(&self, _payload: OnConnectPayload<'_>) -> HookResult { Ok(()) }

    /// Called to authenticate/authorize. Use `context.insert()` to store user info.
    async fn on_authenticate(&self, _payload: OnAuthenticatePayload<'_>) -> HookResult { Ok(()) }

    /// Called when a document is first loaded. Return `Some(bytes)` for persisted state.
    async fn on_load_document(&self, _payload: OnLoadDocumentPayload<'_>) -> Result<Option<Vec<u8>>, Box<dyn Error + Send + Sync>> { Ok(None) }

    /// Called on every document change.
    async fn on_change(&self, _payload: OnChangePayload<'_>) -> HookResult { Ok(()) }

    /// Called when a client disconnects from a document.
    async fn on_disconnect(&self, _payload: OnDisconnectPayload<'_>) -> HookResult { Ok(()) }

    /// Called on explicit save request (control message or HTTP).
    async fn on_save(&self, _payload: OnSavePayload<'_>) -> HookResult { Ok(()) }

    /// Called before a dirty document is unloaded. Use for lazy persistence.
    async fn before_close_dirty(&self, _payload: BeforeCloseDirtyPayload<'_>) -> HookResult { Ok(()) }

    /// Called after a document is fully unloaded from memory.
    fn after_unload_document(&self, _doc_id: &str) {}
}
