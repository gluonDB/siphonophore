use kameo::{
    actor::{Actor, ActorRef, WeakActorRef, ActorId},
    error::{ActorStopReason, Infallible},
    message::{Context as KameoContext, Message, StreamMessage},
};
use axum::extract::ws::{Message as WsMessage, WebSocket};
use futures::{SinkExt, StreamExt, stream::SplitSink};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::ops::ControlFlow;
use std::future::Future;
use tracing::{debug, warn};

use crate::actor::document::DocActor;
use crate::actor::root::Root;
use crate::actor::messages::{RequestDoc, ConnectClient, DisconnectClient, YjsData, WirePayload, PersistNow, SendText};
use crate::payload::{decode_doc_id, encode_with_doc_id};
use crate::hooks::{ Hook, Context, RequestInfo, OnConnectPayload, OnAuthenticatePayload, OnBeforeSyncPayload,BeforeSyncAction, OnControlMessagePayload, ControlMessageResponse };

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum ControlMessage {
    Leave { doc: String },
    Save { doc: String },
}

/// State for a document that's waiting for handshake completion.
struct PendingDoc {
    context: Context,
}

pub struct ClientActorArgs {
    pub socket: WebSocket,
    pub request_info: RequestInfo,
    pub root: ActorRef<Root>,
    pub hooks: Arc<Vec<Box<dyn Hook>>>,
}

pub struct ClientActor {
    sink: SplitSink<WebSocket, WsMessage>,
    /// Active documents (y-sync started).
    docs: HashMap<Arc<str>, ActorRef<DocActor>>,
    /// Documents waiting for handshake (y-sync not started yet).
    pending_docs: HashMap<Arc<str>, PendingDoc>,
    /// Contexts for active documents (needed for control message handling).
    contexts: HashMap<Arc<str>, Context>,
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
            contexts: HashMap::new(),
            root: args.root,
            request_info: args.request_info,
            hooks: args.hooks,
        })
    }

    fn on_link_died(&mut self, _: WeakActorRef<Self>, id: ActorId, _: ActorStopReason) -> impl Future<Output = Result<ControlFlow<ActorStopReason>, Self::Error>> + Send {
        self.docs.retain(|_, doc| doc.id() != id);
        async { Ok(ControlFlow::Continue(())) }
    }
}

impl Message<StreamMessage<Result<WsMessage, axum::Error>, (), &'static str>> for ClientActor {
    type Reply = ();

    async fn handle(&mut self, msg: StreamMessage<Result<WsMessage, axum::Error>, (), &'static str>, ctx: &mut KameoContext<Self, Self::Reply>) {
        match msg {
            StreamMessage::Next(Ok(WsMessage::Binary(data))) => {
                let Some((doc_id, _)) = decode_doc_id(&data) else { return };
                let header_len = 1 + doc_id.len();
                let payload = data.slice(header_len..);
                let doc_id: Arc<str> = doc_id.into();

                let doc = match self.docs.get(&doc_id) {
                    Some(d) => d.clone(),
                    None => match self.connect_doc(Arc::clone(&doc_id), ctx.actor_ref()).await {
                        Some(d) => d,
                        None => return,
                    }
                };
                let _ = doc.tell(YjsData { client_id: ctx.actor_ref().id(), data: payload }).send().await;
            }
            StreamMessage::Next(Ok(WsMessage::Text(text))) => {
                // Try built-in control messages first
                if let Ok(ctrl) = serde_json::from_str::<ControlMessage>(&text) {
                    match ctrl {
                        ControlMessage::Leave { doc } => {
                            let doc_id: Arc<str> = doc.into();
                            self.pending_docs.remove(&doc_id);
                            self.contexts.remove(&doc_id);
                            if let Some(doc_actor) = self.docs.remove(&doc_id) {
                                let _ = doc_actor.tell(DisconnectClient(ctx.actor_ref().clone())).send().await;
                            }
                        }
                        ControlMessage::Save { doc } => {
                            let doc_id: Arc<str> = doc.into();
                            if let Some(doc_actor) = self.docs.get(&doc_id) {
                                let _ = doc_actor.ask(PersistNow).send().await;
                            }
                        }
                    }
                } else {
                    // Not a built-in control message, try hooks
                    self.handle_custom_control_message(&text, ctx.actor_ref().id(), ctx.actor_ref()).await;
                }
            }
            StreamMessage::Next(Ok(WsMessage::Ping(data))) => {
                if self.sink.send(WsMessage::Pong(data)).await.is_err() { ctx.actor_ref().kill(); }
            }
            StreamMessage::Next(Ok(WsMessage::Close(_))) | StreamMessage::Finished(_) => {
                for (_, doc) in self.docs.drain() { let _ = doc.tell(DisconnectClient(ctx.actor_ref().clone())).send().await; }
                ctx.actor_ref().kill();
            }
            StreamMessage::Next(Err(_)) => ctx.actor_ref().kill(),
            _ => {}
        }
    }
}

impl Message<WirePayload> for ClientActor {
    type Reply = ();
    async fn handle(&mut self, msg: WirePayload, ctx: &mut KameoContext<Self, Self::Reply>) {
        if self.sink.send(WsMessage::Binary(msg.0.to_vec().into())).await.is_err() { ctx.actor_ref().kill(); }
    }
}

impl Message<SendText> for ClientActor {
    type Reply = ();
    async fn handle(&mut self, msg: SendText, ctx: &mut KameoContext<Self, Self::Reply>) {
        if self.sink.send(WsMessage::Text(msg.0.into())).await.is_err() { ctx.actor_ref().kill(); }
    }
}

impl ClientActor {
    async fn connect_doc(&mut self, doc_id: Arc<str>, me: &ActorRef<Self>) -> Option<ActorRef<DocActor>> {
        let client_id = me.id();

        // Step 1: on_connect hook
        for hook in self.hooks.iter() {
            if hook.on_connect(OnConnectPayload { doc_id: &doc_id, client_id, request: &self.request_info }).await.is_err() {
                debug!("on_connect hook rejected connection for doc {}", doc_id);
                return None;
            }
        }

        // Step 2: on_authenticate hook
        let mut context = Context::default();
        for hook in self.hooks.iter() {
            if hook.on_authenticate(OnAuthenticatePayload { doc_id: &doc_id, client_id, request: &self.request_info, context: &mut context }).await.is_err() {
                debug!("on_authenticate hook rejected connection for doc {}", doc_id);
                return None;
            }
        }

        // Step 3: on_before_sync hook (for custom handshakes like Files-Ready)
        for hook in self.hooks.iter() {
            match hook.on_before_sync(OnBeforeSyncPayload {
                doc_id: &doc_id,
                client_id,
                request: &self.request_info,
                context: &context,
            }).await {
                Ok(BeforeSyncAction::Continue) => {
                    // Continue to next hook or proceed
                }
                Ok(BeforeSyncAction::SendMessages { messages }) => {
                    // Send messages to client
                    for msg in messages {
                        if self.sink.send(WsMessage::Text(msg.into())).await.is_err() {
                            me.kill();
                            return None;
                        }
                    }
                    // Store as pending - wait for response via on_control_message
                    self.pending_docs.insert(Arc::clone(&doc_id), PendingDoc { context });
                    debug!("Doc {} waiting for handshake completion", doc_id);
                    return None; // Don't connect yet - will be done when handshake completes
                }
                Ok(BeforeSyncAction::Abort { reason }) => {
                    warn!("on_before_sync aborted connection for doc {}: {}", doc_id, reason);
                    return None;
                }
                Err(e) => {
                    warn!("on_before_sync hook failed for doc {}: {}", doc_id, e);
                    return None;
                }
            }
        }
        // Step 4: Connect to document and start y-sync
        self.finish_connect_doc(doc_id, context, me).await
    }

    /// Complete document connection after handshake (if any).
    async fn finish_connect_doc(&mut self, doc_id: Arc<str>, context: Context, me: &ActorRef<Self>) -> Option<ActorRef<DocActor>> {
        let doc = self.root.ask(RequestDoc(Arc::clone(&doc_id))).send().await.ok()?;
        let init_msgs = doc.ask(ConnectClient { client: me.clone(), context: context.clone() }).send().await.ok()?;

        for payload in init_msgs {
            let wire = encode_with_doc_id(&doc_id, &payload);
            if self.sink.send(WsMessage::Binary(wire.to_vec().into())).await.is_err() {
                me.kill();
                return None;
            }
        }

        self.contexts.insert(Arc::clone(&doc_id), context);
        self.docs.insert(doc_id, doc.clone());
        Some(doc)
    }

    /// Complete a pending document connection (called when handshake finishes).
    pub async fn complete_pending_doc(&mut self, doc_id: Arc<str>, me: &ActorRef<Self>) -> Option<ActorRef<DocActor>> {
        if let Some(pending) = self.pending_docs.remove(&doc_id) {
            debug!("Completing pending doc connection for {}", doc_id);
            self.finish_connect_doc(doc_id, pending.context, me).await
        } else {
            None
        }
    }

    /// Handle custom control messages via hooks.
    async fn handle_custom_control_message(&mut self, text: &str, client_id: ActorId, me: &ActorRef<Self>) {
        // First check pending docs (for handshake messages like FilesReady)
        let pending_doc_ids: Vec<Arc<str>> = self.pending_docs.keys().cloned().collect();
        for doc_id in pending_doc_ids {
            if let Some(pending) = self.pending_docs.get(&doc_id) {
                for hook in self.hooks.iter() {
                    let response = hook.on_control_message(OnControlMessagePayload {
                        doc_id: Some(&doc_id),
                        client_id,
                        message: text,
                        context: &pending.context,
                    }).await;

                    match response {
                        ControlMessageResponse::Handled { responses } => {
                            for msg in responses {
                                let _ = self.sink.send(WsMessage::Text(msg.into())).await;
                            }
                            return;
                        }
                        ControlMessageResponse::CompleteHandshake { responses } => {
                            // Send responses first
                            for msg in responses {
                                let _ = self.sink.send(WsMessage::Text(msg.into())).await;
                            }
                            // Complete the pending doc connection
                            let _ = self.complete_pending_doc(Arc::clone(&doc_id), me).await;
                            debug!("Handshake completed for doc {}", doc_id);
                            return;
                        }
                        ControlMessageResponse::NotHandled => continue,
                    }
                }
            }
        }

        // Then check active docs
        let doc_ids: Vec<Arc<str>> = self.contexts.keys().cloned().collect();
        for doc_id in doc_ids {
            if let Some(context) = self.contexts.get(&doc_id) {
                for hook in self.hooks.iter() {
                    let response = hook.on_control_message(OnControlMessagePayload {
                        doc_id: Some(&doc_id),
                        client_id,
                        message: text,
                        context,
                    }).await;

                    match response {
                        ControlMessageResponse::Handled { responses } |
                        ControlMessageResponse::CompleteHandshake { responses } => {
                            for msg in responses {
                                let _ = self.sink.send(WsMessage::Text(msg.into())).await;
                            }
                            return;
                        }
                        ControlMessageResponse::NotHandled => continue,
                    }
                }
            }
        }

        // No doc context - try with None
        for hook in self.hooks.iter() {
            let response = hook.on_control_message(OnControlMessagePayload {
                doc_id: None,
                client_id,
                message: text,
                context: &Context::default(),
            }).await;

            match response {
                ControlMessageResponse::Handled { responses } |
                ControlMessageResponse::CompleteHandshake { responses } => {
                    for msg in responses {
                        let _ = self.sink.send(WsMessage::Text(msg.into())).await;
                    }
                    return;
                }
                ControlMessageResponse::NotHandled => continue,
            }
        }

        debug!("Unhandled control message: {}", text);
    }
}
