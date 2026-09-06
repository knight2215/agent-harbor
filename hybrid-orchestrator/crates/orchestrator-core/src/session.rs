//! Session lifecycle (architecture.md Section 7.5).
//!
//! The `Conversation`, `Message`, and related domain types live in
//! [`crate::models`] (Section 7.1). [`SessionManager`] is the orchestration
//! seam over the [`persistence`] repositories: it creates/lists/renames/tags/
//! pins/deletes conversations, appends messages, and assigns personas.
//!
//! ## Concurrency (Section 7.5)
//!
//! `SessionManager` holds a per-conversation async write lock (`Mutex` keyed by
//! conversation `Uuid`): writes WITHIN one conversation are serialized so a turn
//! cannot interleave with a rename or a second append, while writes to DIFFERENT
//! conversations proceed in parallel (each has its own lock) and reads are never
//! blocked by these locks.
//!
//! At the SQLite layer, [`persistence::Db::open`] enables WAL journaling and a
//! `busy_timeout` so writes to DIFFERENT conversations - which the
//! per-conversation lock does NOT serialize and which may land on different pool
//! connections - wait for the write lock rather than immediately returning
//! `SQLITE_BUSY`. Reads run concurrently under WAL.
//!
//! ## Lock-map growth (Phase 1 scope)
//!
//! `lock_for` lazily inserts one `Arc<Mutex<()>>` per conversation `Uuid`;
//! `delete_conversation` removes the entry for a deleted conversation. Entries
//! are otherwise retained for the life of the manager. Each entry is tiny (a
//! `Uuid` key plus an `Arc` to an empty-tuple mutex, a handful of bytes), and
//! the hybrid orchestrator is a single-user desktop app whose conversation
//! count is bounded by what one user creates and mostly deletes. For Phase 1
//! this bounded, self-limiting growth is acceptable; a general idle-eviction
//! scheme (or an LRU cap) is deferred to a later phase if a long-lived process
//! is observed to touch pathologically many distinct conversations.

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};

use chrono::Utc;
use persistence::{ConversationRepo, Db, MessageRepo, PersistenceError, PersonaRepo};
use tokio::sync::Mutex as AsyncMutex;
use uuid::Uuid;

use crate::models::{Conversation, ManualRoute, Message, PrivacyTag};

/// Errors from session operations.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// Underlying persistence failure.
    #[error(transparent)]
    Persistence(#[from] PersistenceError),
    /// The referenced conversation does not exist.
    #[error("conversation not found: {0}")]
    ConversationNotFound(Uuid),
}

/// Optional initialization for a new conversation.
#[derive(Debug, Clone, Default)]
pub struct ConversationInit {
    pub title: Option<String>,
    pub persona_id: Option<Uuid>,
    pub privacy_tags: Vec<PrivacyTag>,
}

/// Orchestration facade over the persistence repositories with the Section 7.5
/// per-conversation write lock.
#[derive(Clone)]
pub struct SessionManager {
    db: Db,
    /// Per-conversation write locks. The outer `std::Mutex` guards only the map
    /// (held briefly to fetch/insert a lock); the inner `tokio::Mutex` is the
    /// per-conversation lock actually held across an async write. Distinct
    /// conversations get distinct inner locks, so their writes run in parallel.
    conversation_locks: Arc<StdMutex<HashMap<Uuid, Arc<AsyncMutex<()>>>>>,
}

impl SessionManager {
    /// Build a manager over an open [`Db`].
    pub fn new(db: Db) -> Self {
        SessionManager {
            db,
            conversation_locks: Arc::new(StdMutex::new(HashMap::new())),
        }
    }

    /// Borrow the underlying database (reads are concurrent and unlocked).
    pub fn db(&self) -> &Db {
        &self.db
    }

    /// Get (or lazily create) the write lock for a conversation.
    fn lock_for(&self, id: Uuid) -> Arc<AsyncMutex<()>> {
        let mut map = self
            .conversation_locks
            .lock()
            .expect("conversation lock map poisoned");
        map.entry(id)
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone()
    }

    // --- Conversation lifecycle --------------------------------------------

    /// Create and persist a new conversation.
    pub async fn create_conversation(
        &self,
        init: ConversationInit,
    ) -> Result<Conversation, SessionError> {
        let now = Utc::now();
        let conv = Conversation {
            id: Uuid::new_v4(),
            title: init.title.unwrap_or_else(|| "New Conversation".to_string()),
            created_at: now,
            updated_at: now,
            persona_id: init.persona_id,
            conversation_pref: None,
            privacy_tags: init.privacy_tags,
            enabled_tool_servers: Vec::new(),
        };
        // A brand-new id needs no contended lock, but take it anyway so the
        // write path is uniform.
        let lock = self.lock_for(conv.id);
        let _guard = lock.lock().await;
        ConversationRepo::new(&self.db).insert(&conv).await?;
        Ok(conv)
    }

    /// List all conversations (concurrent read; no write lock taken).
    pub async fn list_conversations(&self) -> Result<Vec<Conversation>, SessionError> {
        Ok(ConversationRepo::new(&self.db).list().await?)
    }

    /// Fetch one conversation (concurrent read).
    pub async fn get_conversation(&self, id: Uuid) -> Result<Option<Conversation>, SessionError> {
        Ok(ConversationRepo::new(&self.db).get(id).await?)
    }

    /// Rename a conversation.
    pub async fn rename_conversation(
        &self,
        id: Uuid,
        title: String,
    ) -> Result<Conversation, SessionError> {
        self.mutate_conversation(id, |c| {
            c.title = title;
        })
        .await
    }

    /// Set (or clear) a conversation's privacy tags.
    pub async fn set_conversation_tags(
        &self,
        id: Uuid,
        tags: Vec<PrivacyTag>,
    ) -> Result<Conversation, SessionError> {
        self.mutate_conversation(id, |c| {
            c.privacy_tags = tags;
        })
        .await
    }

    /// Pin (or clear) the per-conversation route (`conversation_pref`).
    pub async fn set_conversation_route(
        &self,
        id: Uuid,
        route: Option<ManualRoute>,
    ) -> Result<Conversation, SessionError> {
        self.mutate_conversation(id, |c| {
            c.conversation_pref = route;
        })
        .await
    }

    /// Assign (or clear) a persona for a conversation.
    pub async fn assign_persona(
        &self,
        id: Uuid,
        persona_id: Option<Uuid>,
    ) -> Result<Conversation, SessionError> {
        self.mutate_conversation(id, |c| {
            c.persona_id = persona_id;
        })
        .await
    }

    /// Delete a conversation (and, via ON DELETE CASCADE, its messages).
    pub async fn delete_conversation(&self, id: Uuid) -> Result<(), SessionError> {
        let lock = self.lock_for(id);
        let _guard = lock.lock().await;
        ConversationRepo::new(&self.db).delete(id).await?;
        // Drop the now-useless lock entry to bound the map's growth.
        if let Ok(mut map) = self.conversation_locks.lock() {
            map.remove(&id);
        }
        Ok(())
    }

    /// Append a message to a conversation, bumping the conversation's
    /// `updated_at`. Serialized against other writes to the SAME conversation
    /// by the per-conversation lock (Section 7.5).
    pub async fn append_message(&self, message: Message) -> Result<Message, SessionError> {
        let conversation_id = message.conversation_id;
        let lock = self.lock_for(conversation_id);
        let _guard = lock.lock().await;

        let conv_repo = ConversationRepo::new(&self.db);
        let mut conv = conv_repo
            .get(conversation_id)
            .await?
            .ok_or(SessionError::ConversationNotFound(conversation_id))?;

        MessageRepo::new(&self.db).insert(&message).await?;

        conv.updated_at = Utc::now();
        conv_repo.update(&conv).await?;
        Ok(message)
    }

    /// List a conversation's messages in order (concurrent read).
    pub async fn list_messages(&self, conversation_id: Uuid) -> Result<Vec<Message>, SessionError> {
        Ok(MessageRepo::new(&self.db)
            .list_for_conversation(conversation_id)
            .await?)
    }

    // --- Persona helpers (exposed for the session surface) ------------------

    /// The persona repository, for the agent-config surface (Section 8.4).
    pub fn personas(&self) -> PersonaRepo<'_> {
        PersonaRepo::new(&self.db)
    }

    // --- Internal -----------------------------------------------------------

    /// Load a conversation under its write lock, apply `f`, bump `updated_at`,
    /// and persist. Centralizes the read-modify-write pattern so every mutator
    /// is serialized per conversation.
    async fn mutate_conversation(
        &self,
        id: Uuid,
        f: impl FnOnce(&mut Conversation),
    ) -> Result<Conversation, SessionError> {
        let lock = self.lock_for(id);
        let _guard = lock.lock().await;

        let repo = ConversationRepo::new(&self.db);
        let mut conv = repo
            .get(id)
            .await?
            .ok_or(SessionError::ConversationNotFound(id))?;
        f(&mut conv);
        conv.updated_at = Utc::now();
        repo.update(&conv).await?;
        Ok(conv)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{AgentPersona, MessageContent, MessageStatus, ModelParameters, Role};

    async fn manager() -> SessionManager {
        SessionManager::new(Db::open_in_memory().await.unwrap())
    }

    fn user_message(conversation_id: Uuid, text: &str) -> Message {
        Message {
            id: Uuid::new_v4(),
            conversation_id,
            role: Role::User,
            content: MessageContent::Text {
                text: text.to_string(),
            },
            created_at: Utc::now(),
            route: None,
            usage: None,
            status: MessageStatus::Complete,
        }
    }

    #[tokio::test]
    async fn session_lifecycle_create_append_assign() {
        let mgr = manager().await;

        // Create + list + rename.
        let conv = mgr
            .create_conversation(ConversationInit {
                title: Some("Chat".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(mgr.list_conversations().await.unwrap().len(), 1);
        let renamed = mgr
            .rename_conversation(conv.id, "Renamed".to_string())
            .await
            .unwrap();
        assert_eq!(renamed.title, "Renamed");

        // Tags + route pin.
        mgr.set_conversation_tags(conv.id, vec![PrivacyTag::Confidential])
            .await
            .unwrap();
        let pinned = mgr
            .set_conversation_route(
                conv.id,
                Some(ManualRoute {
                    provider_id: "lmstudio".to_string(),
                    model: "local".to_string(),
                }),
            )
            .await
            .unwrap();
        assert_eq!(pinned.privacy_tags, vec![PrivacyTag::Confidential]);
        assert_eq!(pinned.conversation_pref.unwrap().provider_id, "lmstudio");

        // Append a message.
        mgr.append_message(user_message(conv.id, "hi"))
            .await
            .unwrap();
        assert_eq!(mgr.list_messages(conv.id).await.unwrap().len(), 1);

        // Assign a persona (persist it first so the id is real).
        let persona = AgentPersona {
            id: Uuid::new_v4(),
            name: "Helper".to_string(),
            system_prompt: "help".to_string(),
            default_route: None,
            routing_hint: None,
            allowed_tool_servers: vec![],
            parameters: ModelParameters::default(),
        };
        mgr.personas().insert(&persona).await.unwrap();
        let assigned = mgr.assign_persona(conv.id, Some(persona.id)).await.unwrap();
        assert_eq!(assigned.persona_id, Some(persona.id));

        // Delete.
        mgr.delete_conversation(conv.id).await.unwrap();
        assert!(mgr.get_conversation(conv.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn appends_to_same_conversation_are_all_persisted() {
        let mgr = manager().await;
        let conv = mgr
            .create_conversation(ConversationInit::default())
            .await
            .unwrap();

        // Concurrent appends to the SAME conversation. The per-conversation
        // write lock serializes them; all must persist without loss or deadlock.
        let mut handles = Vec::new();
        for i in 0..8 {
            let mgr = mgr.clone();
            let id = conv.id;
            handles.push(tokio::spawn(async move {
                mgr.append_message(user_message(id, &format!("msg-{i}")))
                    .await
                    .unwrap();
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
        assert_eq!(mgr.list_messages(conv.id).await.unwrap().len(), 8);
    }

    #[tokio::test]
    async fn writes_to_different_conversations_do_not_deadlock() {
        let mgr = manager().await;
        let a = mgr
            .create_conversation(ConversationInit::default())
            .await
            .unwrap();
        let b = mgr
            .create_conversation(ConversationInit::default())
            .await
            .unwrap();

        // Interleaved writes across two conversations proceed in parallel
        // (distinct locks) and complete.
        let (ra, rb) = tokio::join!(
            {
                let mgr = mgr.clone();
                async move { mgr.append_message(user_message(a.id, "a")).await }
            },
            {
                let mgr = mgr.clone();
                async move { mgr.append_message(user_message(b.id, "b")).await }
            }
        );
        ra.unwrap();
        rb.unwrap();
        assert_eq!(mgr.list_messages(a.id).await.unwrap().len(), 1);
        assert_eq!(mgr.list_messages(b.id).await.unwrap().len(), 1);
    }
}
