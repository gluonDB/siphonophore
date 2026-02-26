use async_trait::async_trait;
use kameo::actor::ActorId;
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::error::Error;
use std::sync::Arc;

pub type HookResult = Result<(), Box<dyn Error + Send + Sync>>;
pub type HookError = Box<dyn Error + Send + Sync>;

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
}

pub struct BeforeCloseDirtyPayload<'a> {
    pub doc_id: &'a str,
    pub state: &'a [u8],
}

/// Payload for the before_sync hook
pub struct OnBeforeSyncPayload<'a> {
    pub doc_id: &'a str,
    pub client_id: ActorId,
    pub request: &'a RequestInfo,
    pub context: &'a Context,
}

/// Action to take after the before_sync hook.
#[derive(Debug, Default)]
pub enum BeforeSyncAction {
    /// Continue with normal y-sync immediately.
    #[default]
    Continue,
    /// Send messages to client, optionally wait for a response, then continue.
    SendMessages {
        /// Text messages to send to the client.
        messages: Vec<String>,
    },
    /// Abort the connection (e.g., handshake failed).
    Abort {
        /// Reason for aborting.
        reason: String,
    },
}

/// Payload for handling custom control messages (text messages not recognized as Leave/Save).
pub struct OnControlMessagePayload<'a> {
    pub doc_id: Option<&'a str>,
    pub client_id: ActorId,
    pub message: &'a str,
    pub context: &'a Context,
}

/// Response from control message handler.
#[derive(Debug, Default)]
pub enum ControlMessageResponse {
    /// Message was not handled by this hook.
    #[default]
    NotHandled,
    /// Message was handled, optionally send response messages.
    Handled {
        /// Optional response messages to send back.
        responses: Vec<String>,
    },
    /// Message completes a pending handshake - send responses and start y-sync.
    ///
    /// Use this when a control message (like "FilesReady") completes the
    /// pre-sync handshake. The responses will be sent, then y-sync will start.
    CompleteHandshake {
        /// Messages to send before starting y-sync (e.g., CrdtState).
        responses: Vec<String>,
    },
}

/// Payload for peer join events.
pub struct OnPeerJoinedPayload<'a> {
    pub doc_id: &'a str,
    pub client_id: ActorId,
    pub context: &'a Context,
    pub peer_count: usize,
}

/// Payload for peer left events.
pub struct OnPeerLeftPayload<'a> {
    pub doc_id: &'a str,
    pub client_id: ActorId,
    pub context: &'a Context,
    pub peer_count: usize,
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

    /// Called after authentication but before y-sync begins.
    async fn on_before_sync(&self, _payload: OnBeforeSyncPayload<'_>) -> Result<BeforeSyncAction, HookError> {
        Ok(BeforeSyncAction::Continue)
    }

    /// Called when a document is first loaded. Return `Some(bytes)` for persisted state.
    async fn on_load_document(&self, _payload: OnLoadDocumentPayload<'_>) -> Result<Option<Vec<u8>>, HookError> { Ok(None) }

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

    /// Called when a text message is received that isn't a built-in control message.
    ///
    /// Return `Handled` if the message was processed, `NotHandled` to pass to next hook.
    async fn on_control_message(&self, _payload: OnControlMessagePayload<'_>) -> ControlMessageResponse {
        ControlMessageResponse::NotHandled
    }

    /// Called when a peer joins a document.
    async fn on_peer_joined(&self, _payload: OnPeerJoinedPayload<'_>) -> HookResult { Ok(()) }

    /// Called when a peer leaves a document.
    async fn on_peer_left(&self, _payload: OnPeerLeftPayload<'_>) -> HookResult { Ok(()) }
}
