use crate::hooks::Hook;
use crate::actor::document::DocActor;
use crate::actor::client::ClientActor;
use kameo::{
    actor::{Actor, ActorRef, WeakActorRef, ActorId, Spawn},
    error::{ActorStopReason, Infallible},
    message::{Context, Message},
};
use std::collections::HashMap;
use std::sync::Arc;
use yrs::Doc;
use std::ops::ControlFlow;
use std::future::Future;

pub struct Root {
    hooks: Arc<Vec<Box<dyn Hook>>>,
    active_docs: HashMap<Arc<str>, ActorRef<DocActor>>,
    clients: HashMap<ActorId, ActorRef<ClientActor>>,
}

impl Root {
    pub fn new() -> Self {
        Self { hooks: Arc::new(vec![]), active_docs: HashMap::new(), clients: HashMap::new() }
    }

    pub fn with_hooks(hooks: Vec<Box<dyn Hook>>) -> Self {
        Self { hooks: Arc::new(hooks), active_docs: HashMap::new(), clients: HashMap::new() }
    }
}

impl Default for Root { fn default() -> Self { Self::new() } }

impl Actor for Root {
    type Args = Self;
    type Error = Infallible;
    
    async fn on_start(state: Self::Args, _: ActorRef<Self>) -> Result<Self, Self::Error> { Ok(state) }
    
    fn on_link_died(&mut self, _: WeakActorRef<Self>, id: ActorId, _reason: ActorStopReason) -> impl Future<Output = Result<ControlFlow<ActorStopReason>, Self::Error>> + Send {
        let doc_id = self.active_docs.iter().find(|(_, a)| a.id() == id).map(|(d, _)| Arc::clone(d));
        if let Some(doc_id) = doc_id {
            self.active_docs.remove(&doc_id);
            for hook in self.hooks.iter() { hook.after_unload_document(&doc_id); }
        }
        self.clients.remove(&id);
        async { Ok(ControlFlow::Continue(())) }
    }
}

use crate::actor::messages::{CreateClient, RequestDoc, PersistDocument, PersistNow, ApplyServerUpdate, GetDocClone, GetServerDoc, DocHandle};
use crate::actor::document::{ApplyUpdate, DocActorArgs};
use crate::actor::client::ClientActorArgs;

impl Message<RequestDoc> for Root {
    type Reply = ActorRef<DocActor>;
    
    async fn handle(&mut self, RequestDoc(doc_id): RequestDoc, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        if let Some(doc) = self.active_docs.get(&doc_id) { return doc.clone(); }
        let doc = DocActor::spawn_link(ctx.actor_ref(), DocActorArgs { doc_id: Arc::clone(&doc_id), hooks: Arc::clone(&self.hooks) }).await;
        self.active_docs.insert(doc_id, doc.clone());
        doc
    }
}

impl Message<CreateClient> for Root {
    type Reply = ActorRef<ClientActor>;
    
    async fn handle(&mut self, msg: CreateClient, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let args = ClientActorArgs { socket: msg.socket, request_info: msg.request_info, root: ctx.actor_ref().clone(), hooks: Arc::clone(&self.hooks) };
        let client = ClientActor::spawn_link(ctx.actor_ref(), args).await;
        self.clients.insert(client.id(), client.clone());
        client
    }
}

impl Message<PersistDocument> for Root {
    type Reply = ();
    async fn handle(&mut self, PersistDocument(doc_id): PersistDocument, _: &mut Context<Self, Self::Reply>) {
        if let Some(doc) = self.active_docs.get(&doc_id) {
            let _ = doc.ask(PersistNow).send().await;
        }
    }
}

impl Message<ApplyServerUpdate> for Root {
    type Reply = bool;
    async fn handle(&mut self, msg: ApplyServerUpdate, _: &mut Context<Self, Self::Reply>) -> bool {
        if let Some(doc) = self.active_docs.get(&msg.doc_id) {
            doc.tell(ApplyUpdate(msg.update)).send().await.is_ok()
        } else { false }
    }
}

impl Message<GetServerDoc> for Root {
    type Reply = DocHandle;
    async fn handle(&mut self, GetServerDoc(doc_id): GetServerDoc, ctx: &mut Context<Self, Self::Reply>) -> DocHandle {
        let doc_actor = if let Some(doc) = self.active_docs.get(&doc_id) {
            doc.clone()
        } else {
            let doc = DocActor::spawn_link(ctx.actor_ref(), DocActorArgs { doc_id: Arc::clone(&doc_id), hooks: Arc::clone(&self.hooks) }).await;
            self.active_docs.insert(doc_id, doc.clone());
            doc
        };
        doc_actor.ask(GetDocClone).send().await.unwrap_or_else(|_| DocHandle(Doc::new()))
    }
}
