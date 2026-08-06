use axum::extract::ws::{Message as WsMessage, WebSocket};
use futures::{stream::SplitSink, SinkExt, StreamExt};
use kameo::{
    actor::{Actor, ActorId, ActorRef, WeakActorRef},
    error::{ActorStopReason, Infallible},
    message::{Context as KameoContext, Message, StreamMessage},
};
use serde::Deserialize;
use std::collections::HashMap;
use std::future::Future;
use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::sleep;
use uuid::Uuid;

use crate::actor::document::DocActor;
use crate::actor::messages::{
    ConnectClient, DisconnectClient, ExpirePending, GetIncarnation, PersistNow, RequestDoc,
    WirePayload, YjsData,
};
use crate::actor::root::Root;
use crate::hooks::{Context, Hook, OnAuthenticatePayload, OnConnectPayload, RequestInfo};
use crate::payload::{decode_doc_id, encode_with_doc_id};

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum ControlMessage {
    Leave { doc: String },
    Save { doc: String },
}

const MSG_INCARNATION: u8 = 2;
const INC_JOIN: u8 = 0;
const INC_INFO: u8 = 1;
const INC_READY: u8 = 2;
const INC_ERROR: u8 = 3;
const ERR_INVALID_MESSAGE: u8 = 1;
const ERR_NOT_READY: u8 = 2;
const ERR_STALE_INCARNATION: u8 = 3;
const ERR_AUTHENTICATION: u8 = 4;
const ERR_LOAD_FAILED: u8 = 5;
const ERR_JOIN_EXPIRED: u8 = 6;
const JOIN_TIMEOUT: Duration = Duration::from_secs(30);

struct PendingDoc {
    doc: ActorRef<DocActor>,
    context: Context,
    incarnation: Uuid,
    expires_at: Instant,
}

pub struct ClientActorArgs {
    pub socket: WebSocket,
    pub request_info: RequestInfo,
    pub root: ActorRef<Root>,
    pub hooks: Arc<Vec<Box<dyn Hook>>>,
}

pub struct ClientActor {
    sink: SplitSink<WebSocket, WsMessage>,
    docs: HashMap<Arc<str>, ActorRef<DocActor>>,
    pending_docs: HashMap<Arc<str>, PendingDoc>,
    root: ActorRef<Root>,
    request_info: RequestInfo,
    hooks: Arc<Vec<Box<dyn Hook>>>,
}

impl Actor for ClientActor {
    type Args = ClientActorArgs;
    type Error = Infallible;

    async fn on_start(args: Self::Args, actor_ref: ActorRef<Self>) -> Result<Self, Self::Error> {
        let (sink, stream) = args.socket.split();
        actor_ref.attach_stream(stream, (), "ws");
        Ok(Self {
            sink,
            docs: HashMap::new(),
            pending_docs: HashMap::new(),
            root: args.root,
            request_info: args.request_info,
            hooks: args.hooks,
        })
    }

    fn on_link_died(
        &mut self,
        _: WeakActorRef<Self>,
        id: ActorId,
        _: ActorStopReason,
    ) -> impl Future<Output = Result<ControlFlow<ActorStopReason>, Self::Error>> + Send {
        self.docs.retain(|_, doc| doc.id() != id);
        self.pending_docs
            .retain(|_, pending| pending.doc.id() != id);
        async { Ok(ControlFlow::Continue(())) }
    }
}

impl Message<StreamMessage<Result<WsMessage, axum::Error>, (), &'static str>> for ClientActor {
    type Reply = ();

    async fn handle(
        &mut self,
        msg: StreamMessage<Result<WsMessage, axum::Error>, (), &'static str>,
        ctx: &mut KameoContext<Self, Self::Reply>,
    ) {
        match msg {
            StreamMessage::Next(Ok(WsMessage::Binary(data))) => {
                let Some((doc_id, _)) = decode_doc_id(&data) else {
                    return;
                };
                let header_len = 1 + doc_id.len();
                let payload = data.slice(header_len..);
                let doc_id: Arc<str> = doc_id.into();
                if payload.first() == Some(&MSG_INCARNATION) {
                    self.handle_incarnation(doc_id, &payload, ctx.actor_ref())
                        .await;
                } else if let Some(doc) = self.docs.get(&doc_id) {
                    let _ = doc
                        .tell(YjsData {
                            client_id: ctx.actor_ref().id(),
                            data: payload,
                        })
                        .send()
                        .await;
                } else {
                    tracing::warn!(doc_id = %doc_id, "rejected document payload before incarnation readiness");
                    self.send_protocol_error(&doc_id, ERR_NOT_READY, ctx.actor_ref())
                        .await;
                }
            }
            StreamMessage::Next(Ok(WsMessage::Text(text))) => {
                if let Ok(ctrl) = serde_json::from_str::<ControlMessage>(&text) {
                    match ctrl {
                        ControlMessage::Leave { doc } => {
                            let doc_id: Arc<str> = doc.into();
                            if let Some(pending) = self.pending_docs.remove(&doc_id) {
                                DocActor::schedule_shutdown(pending.doc);
                            }
                            if let Some(doc_actor) = self.docs.remove(&doc_id) {
                                let _ = doc_actor
                                    .tell(DisconnectClient(ctx.actor_ref().clone()))
                                    .send()
                                    .await;
                            }
                        }
                        ControlMessage::Save { doc } => {
                            let doc_id: Arc<str> = doc.into();
                            if let Some(doc_actor) = self.docs.get(&doc_id) {
                                let _ = doc_actor.ask(PersistNow).send().await;
                            }
                        }
                    }
                }
            }
            StreamMessage::Next(Ok(WsMessage::Ping(data))) => {
                if self.sink.send(WsMessage::Pong(data)).await.is_err() {
                    ctx.actor_ref().kill();
                }
            }
            StreamMessage::Next(Ok(WsMessage::Close(_))) | StreamMessage::Finished(_) => {
                for (_, pending) in self.pending_docs.drain() {
                    DocActor::schedule_shutdown(pending.doc);
                }
                for (_, doc) in self.docs.drain() {
                    let _ = doc
                        .tell(DisconnectClient(ctx.actor_ref().clone()))
                        .send()
                        .await;
                }
                ctx.actor_ref().kill();
            }
            StreamMessage::Next(Err(_)) => {
                for (_, pending) in self.pending_docs.drain() {
                    DocActor::schedule_shutdown(pending.doc);
                }
                ctx.actor_ref().kill();
            }
            _ => {}
        }
    }
}

impl Message<WirePayload> for ClientActor {
    type Reply = ();
    async fn handle(&mut self, msg: WirePayload, ctx: &mut KameoContext<Self, Self::Reply>) {
        if self
            .sink
            .send(WsMessage::Binary(msg.0.to_vec().into()))
            .await
            .is_err()
        {
            ctx.actor_ref().kill();
        }
    }
}

impl Message<ExpirePending> for ClientActor {
    type Reply = ();
    async fn handle(&mut self, msg: ExpirePending, _: &mut KameoContext<Self, Self::Reply>) {
        let should_expire = self.pending_docs.get(&msg.doc_id).is_some_and(|pending| {
            pending.incarnation == msg.incarnation && pending.expires_at <= Instant::now()
        });
        if should_expire {
            if let Some(pending) = self.pending_docs.remove(&msg.doc_id) {
                DocActor::schedule_shutdown(pending.doc);
            }
        }
    }
}

impl ClientActor {
    async fn send_payload(&mut self, doc_id: &str, payload: &[u8], me: &ActorRef<Self>) {
        let wire = encode_with_doc_id(doc_id, payload);
        if self
            .sink
            .send(WsMessage::Binary(wire.to_vec().into()))
            .await
            .is_err()
        {
            me.kill();
        }
    }

    async fn send_protocol_error(&mut self, doc_id: &str, code: u8, me: &ActorRef<Self>) {
        self.send_payload(doc_id, &[MSG_INCARNATION, INC_ERROR, code], me)
            .await;
    }

    async fn begin_join(&mut self, doc_id: Arc<str>, me: &ActorRef<Self>) {
        let client_id = me.id();

        for hook in self.hooks.iter() {
            if hook
                .on_connect(OnConnectPayload {
                    doc_id: &doc_id,
                    client_id,
                    request: &self.request_info,
                })
                .await
                .is_err()
            {
                self.send_protocol_error(&doc_id, ERR_AUTHENTICATION, me)
                    .await;
                return;
            }
        }

        let mut context = Context::default();
        for hook in self.hooks.iter() {
            if hook
                .on_authenticate(OnAuthenticatePayload {
                    doc_id: &doc_id,
                    client_id,
                    request: &self.request_info,
                    context: &mut context,
                })
                .await
                .is_err()
            {
                self.send_protocol_error(&doc_id, ERR_AUTHENTICATION, me)
                    .await;
                return;
            }
        }

        let Ok(doc) = self.root.ask(RequestDoc(Arc::clone(&doc_id))).send().await else {
            self.send_protocol_error(&doc_id, ERR_LOAD_FAILED, me).await;
            return;
        };
        let Ok(status) = doc.ask(GetIncarnation).send().await else {
            self.send_protocol_error(&doc_id, ERR_LOAD_FAILED, me).await;
            return;
        };
        if status.1 {
            tracing::error!(doc_id = %doc_id, "document load failed during incarnation join");
            self.send_protocol_error(&doc_id, ERR_LOAD_FAILED, me).await;
            DocActor::schedule_shutdown(doc);
            return;
        }

        let expires_at = Instant::now() + JOIN_TIMEOUT;
        let incarnation = Uuid::from_bytes(status.0);
        tracing::debug!(doc_id = %doc_id, %incarnation, "offering document incarnation");
        self.pending_docs.insert(
            Arc::clone(&doc_id),
            PendingDoc {
                doc,
                context,
                incarnation,
                expires_at,
            },
        );
        let mut info = Vec::with_capacity(18);
        info.extend_from_slice(&[MSG_INCARNATION, INC_INFO]);
        info.extend_from_slice(incarnation.as_bytes());
        self.send_payload(&doc_id, &info, me).await;

        let me = me.clone();
        tokio::spawn(async move {
            sleep(JOIN_TIMEOUT).await;
            let _ = me
                .tell(ExpirePending {
                    doc_id,
                    incarnation,
                })
                .send()
                .await;
        });
    }

    async fn finish_join(&mut self, doc_id: Arc<str>, incarnation: Uuid, me: &ActorRef<Self>) {
        let Some(pending) = self.pending_docs.remove(&doc_id) else {
            self.send_protocol_error(&doc_id, ERR_JOIN_EXPIRED, me)
                .await;
            return;
        };
        if pending.expires_at <= Instant::now() {
            DocActor::schedule_shutdown(pending.doc);
            self.send_protocol_error(&doc_id, ERR_JOIN_EXPIRED, me)
                .await;
            return;
        }
        let Ok(status) = pending.doc.ask(GetIncarnation).send().await else {
            self.send_protocol_error(&doc_id, ERR_LOAD_FAILED, me).await;
            return;
        };
        if status.1 {
            self.send_protocol_error(&doc_id, ERR_LOAD_FAILED, me).await;
            return;
        }
        if incarnation != pending.incarnation || incarnation.as_bytes() != &status.0 {
            tracing::warn!(doc_id = %doc_id, offered = %pending.incarnation, received = %incarnation, "rejected stale incarnation readiness");
            self.send_protocol_error(&doc_id, ERR_STALE_INCARNATION, me)
                .await;
            return;
        }

        let Ok(init_msgs) = pending
            .doc
            .ask(ConnectClient {
                client: me.clone(),
                context: pending.context,
            })
            .send()
            .await
        else {
            self.send_protocol_error(&doc_id, ERR_LOAD_FAILED, me).await;
            return;
        };

        for payload in init_msgs {
            self.send_payload(&doc_id, &payload, me).await;
        }

        self.docs.insert(doc_id, pending.doc);
    }

    async fn handle_incarnation(&mut self, doc_id: Arc<str>, payload: &[u8], me: &ActorRef<Self>) {
        match payload.get(1).copied() {
            Some(INC_JOIN) if payload.len() == 2 || payload.len() == 18 => {
                if let Some(doc) = self.docs.remove(&doc_id) {
                    let _ = doc.tell(DisconnectClient(me.clone())).send().await;
                }
                if let Some(pending) = self.pending_docs.remove(&doc_id) {
                    DocActor::schedule_shutdown(pending.doc);
                }
                self.begin_join(doc_id, me).await;
            }
            Some(INC_READY) if payload.len() == 18 => {
                let Ok(bytes) = <[u8; 16]>::try_from(&payload[2..18]) else {
                    self.send_protocol_error(&doc_id, ERR_INVALID_MESSAGE, me)
                        .await;
                    return;
                };
                self.finish_join(doc_id, Uuid::from_bytes(bytes), me).await;
            }
            _ => {
                self.send_protocol_error(&doc_id, ERR_INVALID_MESSAGE, me)
                    .await
            }
        }
    }
}
