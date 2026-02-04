use axum::extract::ws::WebSocket;
use bytes::Bytes;
use kameo::actor::{ActorId, ActorRef};
use std::sync::Arc;
use crate::actor::client::ClientActor;
use crate::hooks::{Context, RequestInfo};

pub struct CreateClient {
    pub socket: WebSocket,
    pub request_info: RequestInfo,
}

pub struct RequestDoc(pub Arc<str>);

pub struct ConnectClient {
    pub client: ActorRef<ClientActor>,
    pub context: Context,
}

pub struct DisconnectClient(pub ActorRef<ClientActor>);

pub struct YjsData {
    pub client_id: ActorId,
    pub data: Bytes,
}

pub struct WirePayload(pub Bytes);

pub struct IdleShutdown;

pub struct PersistNow;

pub struct PersistDocument(pub Arc<str>);

pub struct ApplyServerUpdate {
    pub doc_id: Arc<str>,
    pub update: Vec<u8>,
}

/// Send a text message to a specific client.
pub struct SendText(pub String);

/// Broadcast a text message to all clients in a document.
pub struct BroadcastText {
    pub doc_id: Arc<str>,
    pub message: String,
    /// If Some, exclude this client from the broadcast.
    pub exclude_client: Option<ActorId>,
}
