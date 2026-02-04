use kameo::{
    actor::{Actor, ActorRef, WeakActorRef, ActorId},
    error::{ActorStopReason, Infallible},
    message::{Context as KameoContext, Message},
};
use tokio::time::{sleep, Duration};
use yrs::{
    Doc, ReadTxn, StateVector, Transact, Update,
    sync::{Awareness, Message as YMessage},
    updates::encoder::{Encode, Encoder, EncoderV1},
    updates::decoder::{Decode, DecoderV1},
    sync::protocol::{DefaultProtocol, AsyncProtocol, MessageReader},
    encoding::read::Cursor,
    encoding::write::Write,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::ops::ControlFlow;
use std::future::Future;

use crate::hooks::{Hook, Context, OnLoadDocumentPayload, OnChangePayload, OnDisconnectPayload, OnSavePayload, BeforeCloseDirtyPayload, OnPeerJoinedPayload, OnPeerLeftPayload};
use crate::actor::client::ClientActor;
use crate::actor::messages::{IdleShutdown, YjsData, ConnectClient, DisconnectClient, WirePayload, PersistNow, SendText, BroadcastText};
use crate::payload::encode_with_doc_id;

const MSG_SYNC: u8 = 0;
const MSG_AWARENESS: u8 = 1;
const SYNC_UPDATE: u8 = 2;
const WS_TOLERANCE: Duration = Duration::from_secs(10);

pub struct DocActor {
    doc_id: Arc<str>,
    doc: Doc,
    awareness: Awareness,
    protocol: DefaultProtocol,
    clients: HashMap<ActorId, ActorRef<ClientActor>>,
    contexts: HashMap<ActorId, Context>,
    client_awareness_ids: HashMap<ActorId, u64>,
    dirty: bool,
    hooks: Arc<Vec<Box<dyn Hook>>>,
}

pub struct DocActorArgs {
    pub doc_id: Arc<str>,
    pub hooks: Arc<Vec<Box<dyn Hook>>>,
}

impl DocActor {
    fn handle_client_left(&mut self, client_id: ActorId) -> bool {
        if let Some(awareness_id) = self.client_awareness_ids.remove(&client_id) { self.awareness.remove_state(awareness_id); }
        self.clients.remove(&client_id);
        let peer_count = self.clients.len();

        if let Some(context) = self.contexts.remove(&client_id) {
            let doc_id = Arc::clone(&self.doc_id);
            let hooks = Arc::clone(&self.hooks);
            tokio::spawn(async move {
                for hook in hooks.iter() {
                    let _ = hook.on_disconnect(OnDisconnectPayload { doc_id: &doc_id, client_id, context: &context }).await;
                    let _ = hook.on_peer_left(OnPeerLeftPayload { doc_id: &doc_id, client_id, context: &context, peer_count }).await;
                }
            });
        }
        self.clients.is_empty()
    }

    /// Broadcast a text message to all clients (or all except one).
    fn broadcast_text(&self, message: &str, exclude_client: Option<ActorId>) {
        for (id, client) in &self.clients {
            if exclude_client.map_or(true, |exc| *id != exc) {
                let msg = message.to_string();
                let c = client.clone();
                tokio::spawn(async move { let _ = c.tell(SendText(msg)).send().await; });
            }
        }
    }
    fn schedule_shutdown(actor: ActorRef<Self>, _doc_id: Arc<str>) {
        tokio::spawn(async move { sleep(WS_TOLERANCE).await; let _ = actor.tell(IdleShutdown).send().await; });
    }
    fn broadcast(&self, sender_id: ActorId, payload: &[u8]) {
        let wire = encode_with_doc_id(&self.doc_id, payload);
        for (id, client) in &self.clients {
            if *id != sender_id {
                let w = wire.clone();
                let c = client.clone();
                tokio::spawn(async move { let _ = c.tell(WirePayload(w)).send().await; });
            }
        }
    }

    fn should_broadcast(data: &[u8]) -> bool {
        matches!(data.first(), Some(&MSG_AWARENESS)) || (data.first() == Some(&MSG_SYNC) && data.get(1) == Some(&SYNC_UPDATE))
    }

    fn is_doc_change(data: &[u8]) -> bool {
        data.first() == Some(&MSG_SYNC) && matches!(data.get(1), Some(&1) | Some(&2))
    }

    fn decode_messages(data: &[u8]) -> Vec<YMessage> {
        let mut decoder = DecoderV1::new(Cursor::new(data));
        let mut reader = MessageReader::new(&mut decoder);
        let mut msgs = Vec::new();
        while let Some(Ok(m)) = reader.next() { msgs.push(m); }
        msgs
    }
}

impl Actor for DocActor {
    type Args = DocActorArgs;
    type Error = Infallible;
    
    async fn on_start(args: Self::Args, _: ActorRef<Self>) -> Result<Self, Self::Error> {
        let doc = Doc::new();
        for hook in args.hooks.iter() {
            match hook.on_load_document(OnLoadDocumentPayload { doc_id: &args.doc_id }).await {
                Ok(Some(data)) => {
                    if let Ok(update) = Update::decode_v1(&data) {
                        let _ = doc.transact_mut().apply_update(update);
                    }
                    break;
                }
                Ok(None) => continue,
                Err(_) => continue,
            }
        }
        Ok(Self { doc_id: args.doc_id, awareness: Awareness::new(doc.clone()), doc, protocol: DefaultProtocol, clients: HashMap::new(), contexts: HashMap::new(), client_awareness_ids: HashMap::new(), dirty: false, hooks: args.hooks })
    }

    fn on_link_died(&mut self, actor_ref: WeakActorRef<Self>, id: ActorId, _: ActorStopReason) -> impl Future<Output = Result<ControlFlow<ActorStopReason>, Self::Error>> + Send {
        let should_shutdown = self.handle_client_left(id);
        let doc_id = Arc::clone(&self.doc_id);
        async move {
            if should_shutdown {
                if let Some(actor) = actor_ref.upgrade() {
                    Self::schedule_shutdown(actor, doc_id);
                }
            }
            Ok(ControlFlow::Continue(()))
        }
    }
}

impl Message<IdleShutdown> for DocActor {
    type Reply = ();
    async fn handle(&mut self, _: IdleShutdown, ctx: &mut KameoContext<Self, Self::Reply>) {
        if !self.clients.is_empty() { return; }
        if self.dirty {
            let state = self.doc.transact().encode_state_as_update_v1(&StateVector::default());
            for hook in self.hooks.iter() {
                let _ = hook.before_close_dirty(BeforeCloseDirtyPayload { doc_id: &self.doc_id, state: &state }).await;
            }
        }
        ctx.actor_ref().kill();
    }
}

impl Message<PersistNow> for DocActor {
    type Reply = ();
    async fn handle(&mut self, _: PersistNow, _: &mut KameoContext<Self, Self::Reply>) {
        let state = self.doc.transact().encode_state_as_update_v1(&StateVector::default());
        for hook in self.hooks.iter() {
            if hook.on_save(OnSavePayload { doc_id: &self.doc_id, state: &state }).await.is_err() { return; }
        }
        self.dirty = false;
    }
}

impl Message<ConnectClient> for DocActor {
    type Reply = Vec<Vec<u8>>;
    async fn handle(&mut self, msg: ConnectClient, ctx: &mut KameoContext<Self, Self::Reply>) -> Self::Reply {
        ctx.actor_ref().link(&msg.client).await;
        let client_id = msg.client.id();
        self.clients.insert(client_id, msg.client);
        self.contexts.insert(client_id, msg.context.clone());

        // Notify hooks about peer joining
        let peer_count = self.clients.len();
        let doc_id = Arc::clone(&self.doc_id);
        let hooks = Arc::clone(&self.hooks);
        let context = msg.context;
        tokio::spawn(async move {
            for hook in hooks.iter() {
                let _ = hook.on_peer_joined(OnPeerJoinedPayload {
                    doc_id: &doc_id,
                    client_id,
                    context: &context,
                    peer_count,
                }).await;
            }
        });

        self.protocol.start::<EncoderV1>(&self.awareness).await.map(|msgs| msgs.into_iter().map(|m| m.encode_v1()).collect()).unwrap_or_default()
    }
}

impl Message<DisconnectClient> for DocActor {
    type Reply = ();
    async fn handle(&mut self, DisconnectClient(client): DisconnectClient, ctx: &mut KameoContext<Self, Self::Reply>) {
        ctx.actor_ref().unlink(&client).await;
        if self.handle_client_left(client.id()) { Self::schedule_shutdown(ctx.actor_ref().clone(), Arc::clone(&self.doc_id)); }
    }
}

impl Message<YjsData> for DocActor {
    type Reply = ();
    async fn handle(&mut self, msg: YjsData, _: &mut KameoContext<Self, Self::Reply>) {
        let data = &msg.data[..];
        let responses = match self.protocol.handle(&self.awareness, data).await { Ok(r) => r, Err(_) => return };
        
        if let Some(client) = self.clients.get(&msg.client_id) {
            for resp in responses {
                let wire = encode_with_doc_id(&self.doc_id, &resp.encode_v1());
                let _ = client.tell(WirePayload(wire)).send().await;
            }
        }
        
        if Self::is_doc_change(data) {
            self.dirty = true;
            if let Some(context) = self.contexts.get(&msg.client_id) {
                for hook in self.hooks.iter() {
                    let _ = hook.on_change(OnChangePayload { doc_id: &self.doc_id, client_id: msg.client_id, update: data, context }).await;
                }
            }
        }
        
        if !self.client_awareness_ids.contains_key(&msg.client_id) {
            for m in Self::decode_messages(data) {
                if let YMessage::Awareness(update) = m {
                    if let Some((&id, _)) = update.clients.iter().next() {
                        self.client_awareness_ids.insert(msg.client_id, id);
                        break;
                    }
                }
            }
        }
        
        if Self::should_broadcast(data) { self.broadcast(msg.client_id, data); }
    }
}

pub struct ApplyUpdate(pub Vec<u8>);

impl Message<ApplyUpdate> for DocActor {
    type Reply = ();
    async fn handle(&mut self, ApplyUpdate(update): ApplyUpdate, _: &mut KameoContext<Self, Self::Reply>) {
        if self.doc.transact_mut().apply_update(Update::decode_v1(&update).unwrap()).is_err() { return; }
        self.dirty = true;
        let mut encoder = EncoderV1::new();
        encoder.write_var(MSG_SYNC as u32);
        encoder.write_var(SYNC_UPDATE as u32);
        encoder.write_buf(&update);
        let wire = encode_with_doc_id(&self.doc_id, &encoder.to_vec());
        for client in self.clients.values() {
            let w = wire.clone();
            let c = client.clone();
            tokio::spawn(async move { let _ = c.tell(WirePayload(w)).send().await; });
        }
    }
}

impl Message<BroadcastText> for DocActor {
    type Reply = ();
    async fn handle(&mut self, msg: BroadcastText, _: &mut KameoContext<Self, Self::Reply>) {
        self.broadcast_text(&msg.message, msg.exclude_client);
    }
}

/// Get the number of connected clients.
pub struct GetPeerCount;

impl Message<GetPeerCount> for DocActor {
    type Reply = usize;
    async fn handle(&mut self, _: GetPeerCount, _: &mut KameoContext<Self, Self::Reply>) -> Self::Reply {
        self.clients.len()
    }
}
