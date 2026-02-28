use axum::extract::ws::WebSocket;
use bytes::Bytes;
use kameo::actor::{ActorId, ActorRef};
use kameo::error::Infallible;
use kameo::reply::{Reply, ReplyError};
use std::sync::Arc;
use yrs::Doc;
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

pub struct GetDocClone;

pub struct GetServerDoc(pub Arc<str>);

/// Newtype wrapper around `yrs::Doc` that implements kameo's `Reply` trait.
pub struct DocHandle(pub Doc);

impl Reply for DocHandle {
    type Ok = Self;
    type Error = Infallible;
    type Value = Self;

    fn to_result(self) -> Result<Self, Infallible> { Ok(self) }
    fn into_any_err(self) -> Option<Box<dyn ReplyError>> { None }
    fn into_value(self) -> Self::Value { self }
}
