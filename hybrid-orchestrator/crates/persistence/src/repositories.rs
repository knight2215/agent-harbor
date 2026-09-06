//! Typed repositories over the Section 7.1 domain models (architecture.md
//! Section 7.2).
//!
//! Each repository borrows the [`Db`] pool and offers `insert` / `get` / `list`
//! / `update` / `delete`. Structured fields (privacy tags, tool-server lists,
//! message content, routes, usage, transports, parameters, provider extras) are
//! stored as JSON TEXT columns via `serde_json`; `Uuid`s and timestamps are
//! stored as their string forms. Secrets are NEVER stored: the providers repo
//! persists only the `SecretRef` handle string (Section 7.2 / 9.1).
//!
//! Uses runtime `sqlx::query(...)` APIs, not the compile-time `query!` macros,
//! so no `DATABASE_URL` is needed at build time.

use chrono::{DateTime, Utc};
use orchestrator_core::{
    AgentPersona, Conversation, ManualRoute, McpServerConfig, McpTransport, Message,
    MessageContent, MessageStatus, ModelParameters, PermissionMode, PrivacyTag, ProviderConfig,
    ProviderKind, Role, RouteMetadata, RoutingHint, SecretRef, TokenUsage,
};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::db::{Db, PersistenceError};

type Result<T> = std::result::Result<T, PersistenceError>;

/// Serialize a value to a JSON string for a TEXT column.
fn to_json<T: serde::Serialize>(value: &T) -> Result<String> {
    Ok(serde_json::to_string(value)?)
}

/// Deserialize a JSON string from a TEXT column.
fn from_json<T: serde::de::DeserializeOwned>(s: &str) -> Result<T> {
    Ok(serde_json::from_str(s)?)
}

// --- Conversations ----------------------------------------------------------

/// CRUD for [`Conversation`] rows.
pub struct ConversationRepo<'a> {
    pool: &'a SqlitePool,
}

impl<'a> ConversationRepo<'a> {
    pub fn new(db: &'a Db) -> Self {
        Self { pool: db.pool() }
    }

    pub async fn insert(&self, c: &Conversation) -> Result<()> {
        sqlx::query(
            "INSERT INTO conversations (id, title, created_at, updated_at, persona_id, \
             conversation_pref, privacy_tags, enabled_tool_servers) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(c.id.to_string())
        .bind(&c.title)
        .bind(c.created_at.to_rfc3339())
        .bind(c.updated_at.to_rfc3339())
        .bind(c.persona_id.map(|id| id.to_string()))
        .bind(opt_to_json(&c.conversation_pref)?)
        .bind(to_json(&c.privacy_tags)?)
        .bind(to_json(&c.enabled_tool_servers)?)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    pub async fn get(&self, id: Uuid) -> Result<Option<Conversation>> {
        let row = sqlx::query("SELECT * FROM conversations WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(self.pool)
            .await?;
        row.map(|r| row_to_conversation(&r)).transpose()
    }

    pub async fn list(&self) -> Result<Vec<Conversation>> {
        let rows = sqlx::query("SELECT * FROM conversations ORDER BY updated_at DESC")
            .fetch_all(self.pool)
            .await?;
        rows.iter().map(row_to_conversation).collect()
    }

    pub async fn update(&self, c: &Conversation) -> Result<()> {
        let res = sqlx::query(
            "UPDATE conversations SET title = ?, updated_at = ?, persona_id = ?, \
             conversation_pref = ?, privacy_tags = ?, enabled_tool_servers = ? WHERE id = ?",
        )
        .bind(&c.title)
        .bind(c.updated_at.to_rfc3339())
        .bind(c.persona_id.map(|id| id.to_string()))
        .bind(opt_to_json(&c.conversation_pref)?)
        .bind(to_json(&c.privacy_tags)?)
        .bind(to_json(&c.enabled_tool_servers)?)
        .bind(c.id.to_string())
        .execute(self.pool)
        .await?;
        if res.rows_affected() == 0 {
            return Err(PersistenceError::NotFound(format!("conversation {}", c.id)));
        }
        Ok(())
    }

    pub async fn delete(&self, id: Uuid) -> Result<()> {
        sqlx::query("DELETE FROM conversations WHERE id = ?")
            .bind(id.to_string())
            .execute(self.pool)
            .await?;
        Ok(())
    }
}

fn row_to_conversation(r: &sqlx::sqlite::SqliteRow) -> Result<Conversation> {
    let persona_id: Option<String> = r.try_get("persona_id")?;
    let pref: Option<String> = r.try_get("conversation_pref")?;
    Ok(Conversation {
        id: parse_uuid(r.try_get::<String, _>("id")?)?,
        title: r.try_get("title")?,
        created_at: parse_dt(r.try_get::<String, _>("created_at")?)?,
        updated_at: parse_dt(r.try_get::<String, _>("updated_at")?)?,
        persona_id: persona_id.map(parse_uuid).transpose()?,
        conversation_pref: opt_from_json::<ManualRoute>(pref)?,
        privacy_tags: from_json::<Vec<PrivacyTag>>(&r.try_get::<String, _>("privacy_tags")?)?,
        enabled_tool_servers: from_json::<Vec<Uuid>>(
            &r.try_get::<String, _>("enabled_tool_servers")?,
        )?,
    })
}

// --- Messages ---------------------------------------------------------------

/// CRUD for [`Message`] rows.
pub struct MessageRepo<'a> {
    pool: &'a SqlitePool,
}

impl<'a> MessageRepo<'a> {
    pub fn new(db: &'a Db) -> Self {
        Self { pool: db.pool() }
    }

    pub async fn insert(&self, m: &Message) -> Result<()> {
        sqlx::query(
            "INSERT INTO messages (id, conversation_id, role, content, created_at, route, usage, \
             status) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(m.id.to_string())
        .bind(m.conversation_id.to_string())
        .bind(to_json(&m.role)?)
        .bind(to_json(&m.content)?)
        .bind(m.created_at.to_rfc3339())
        .bind(opt_to_json(&m.route)?)
        .bind(opt_to_json(&m.usage)?)
        .bind(to_json(&m.status)?)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    pub async fn get(&self, id: Uuid) -> Result<Option<Message>> {
        let row = sqlx::query("SELECT * FROM messages WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(self.pool)
            .await?;
        row.map(|r| row_to_message(&r)).transpose()
    }

    /// List messages for a conversation in creation order.
    pub async fn list_for_conversation(&self, conversation_id: Uuid) -> Result<Vec<Message>> {
        let rows = sqlx::query(
            "SELECT * FROM messages WHERE conversation_id = ? ORDER BY created_at ASC, id ASC",
        )
        .bind(conversation_id.to_string())
        .fetch_all(self.pool)
        .await?;
        rows.iter().map(row_to_message).collect()
    }

    pub async fn update(&self, m: &Message) -> Result<()> {
        let res = sqlx::query(
            "UPDATE messages SET role = ?, content = ?, route = ?, usage = ?, status = ? \
             WHERE id = ?",
        )
        .bind(to_json(&m.role)?)
        .bind(to_json(&m.content)?)
        .bind(opt_to_json(&m.route)?)
        .bind(opt_to_json(&m.usage)?)
        .bind(to_json(&m.status)?)
        .bind(m.id.to_string())
        .execute(self.pool)
        .await?;
        if res.rows_affected() == 0 {
            return Err(PersistenceError::NotFound(format!("message {}", m.id)));
        }
        Ok(())
    }

    pub async fn delete(&self, id: Uuid) -> Result<()> {
        sqlx::query("DELETE FROM messages WHERE id = ?")
            .bind(id.to_string())
            .execute(self.pool)
            .await?;
        Ok(())
    }
}

fn row_to_message(r: &sqlx::sqlite::SqliteRow) -> Result<Message> {
    let route: Option<String> = r.try_get("route")?;
    let usage: Option<String> = r.try_get("usage")?;
    Ok(Message {
        id: parse_uuid(r.try_get::<String, _>("id")?)?,
        conversation_id: parse_uuid(r.try_get::<String, _>("conversation_id")?)?,
        role: from_json::<Role>(&r.try_get::<String, _>("role")?)?,
        content: from_json::<MessageContent>(&r.try_get::<String, _>("content")?)?,
        created_at: parse_dt(r.try_get::<String, _>("created_at")?)?,
        route: opt_from_json::<RouteMetadata>(route)?,
        usage: opt_from_json::<TokenUsage>(usage)?,
        status: from_json::<MessageStatus>(&r.try_get::<String, _>("status")?)?,
    })
}

// --- Personas ---------------------------------------------------------------

/// CRUD for [`AgentPersona`] rows.
pub struct PersonaRepo<'a> {
    pool: &'a SqlitePool,
}

impl<'a> PersonaRepo<'a> {
    pub fn new(db: &'a Db) -> Self {
        Self { pool: db.pool() }
    }

    pub async fn insert(&self, p: &AgentPersona) -> Result<()> {
        sqlx::query(
            "INSERT INTO agent_personas (id, name, system_prompt, default_route, routing_hint, \
             allowed_tool_servers, parameters) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(p.id.to_string())
        .bind(&p.name)
        .bind(&p.system_prompt)
        .bind(opt_to_json(&p.default_route)?)
        .bind(opt_to_json(&p.routing_hint)?)
        .bind(to_json(&p.allowed_tool_servers)?)
        .bind(to_json(&p.parameters)?)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    pub async fn get(&self, id: Uuid) -> Result<Option<AgentPersona>> {
        let row = sqlx::query("SELECT * FROM agent_personas WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(self.pool)
            .await?;
        row.map(|r| row_to_persona(&r)).transpose()
    }

    pub async fn list(&self) -> Result<Vec<AgentPersona>> {
        let rows = sqlx::query("SELECT * FROM agent_personas ORDER BY name ASC")
            .fetch_all(self.pool)
            .await?;
        rows.iter().map(row_to_persona).collect()
    }

    pub async fn update(&self, p: &AgentPersona) -> Result<()> {
        let res = sqlx::query(
            "UPDATE agent_personas SET name = ?, system_prompt = ?, default_route = ?, \
             routing_hint = ?, allowed_tool_servers = ?, parameters = ? WHERE id = ?",
        )
        .bind(&p.name)
        .bind(&p.system_prompt)
        .bind(opt_to_json(&p.default_route)?)
        .bind(opt_to_json(&p.routing_hint)?)
        .bind(to_json(&p.allowed_tool_servers)?)
        .bind(to_json(&p.parameters)?)
        .bind(p.id.to_string())
        .execute(self.pool)
        .await?;
        if res.rows_affected() == 0 {
            return Err(PersistenceError::NotFound(format!("persona {}", p.id)));
        }
        Ok(())
    }

    pub async fn delete(&self, id: Uuid) -> Result<()> {
        sqlx::query("DELETE FROM agent_personas WHERE id = ?")
            .bind(id.to_string())
            .execute(self.pool)
            .await?;
        Ok(())
    }
}

fn row_to_persona(r: &sqlx::sqlite::SqliteRow) -> Result<AgentPersona> {
    let default_route: Option<String> = r.try_get("default_route")?;
    let routing_hint: Option<String> = r.try_get("routing_hint")?;
    Ok(AgentPersona {
        id: parse_uuid(r.try_get::<String, _>("id")?)?,
        name: r.try_get("name")?,
        system_prompt: r.try_get("system_prompt")?,
        default_route: opt_from_json::<ManualRoute>(default_route)?,
        routing_hint: opt_from_json::<RoutingHint>(routing_hint)?,
        allowed_tool_servers: from_json::<Vec<Uuid>>(
            &r.try_get::<String, _>("allowed_tool_servers")?,
        )?,
        parameters: from_json::<ModelParameters>(&r.try_get::<String, _>("parameters")?)?,
    })
}

// --- MCP servers ------------------------------------------------------------

/// CRUD for [`McpServerConfig`] rows.
pub struct McpServerRepo<'a> {
    pool: &'a SqlitePool,
}

impl<'a> McpServerRepo<'a> {
    pub fn new(db: &'a Db) -> Self {
        Self { pool: db.pool() }
    }

    pub async fn insert(&self, s: &McpServerConfig) -> Result<()> {
        sqlx::query(
            "INSERT INTO mcp_servers (id, name, transport, permission_mode, enabled) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(s.id.to_string())
        .bind(&s.name)
        .bind(to_json(&s.transport)?)
        .bind(to_json(&s.permission_mode)?)
        .bind(s.enabled as i64)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    pub async fn get(&self, id: Uuid) -> Result<Option<McpServerConfig>> {
        let row = sqlx::query("SELECT * FROM mcp_servers WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(self.pool)
            .await?;
        row.map(|r| row_to_mcp_server(&r)).transpose()
    }

    pub async fn list(&self) -> Result<Vec<McpServerConfig>> {
        let rows = sqlx::query("SELECT * FROM mcp_servers ORDER BY name ASC")
            .fetch_all(self.pool)
            .await?;
        rows.iter().map(row_to_mcp_server).collect()
    }

    pub async fn update(&self, s: &McpServerConfig) -> Result<()> {
        let res = sqlx::query(
            "UPDATE mcp_servers SET name = ?, transport = ?, permission_mode = ?, enabled = ? \
             WHERE id = ?",
        )
        .bind(&s.name)
        .bind(to_json(&s.transport)?)
        .bind(to_json(&s.permission_mode)?)
        .bind(s.enabled as i64)
        .bind(s.id.to_string())
        .execute(self.pool)
        .await?;
        if res.rows_affected() == 0 {
            return Err(PersistenceError::NotFound(format!("mcp_server {}", s.id)));
        }
        Ok(())
    }

    pub async fn delete(&self, id: Uuid) -> Result<()> {
        sqlx::query("DELETE FROM mcp_servers WHERE id = ?")
            .bind(id.to_string())
            .execute(self.pool)
            .await?;
        Ok(())
    }
}

fn row_to_mcp_server(r: &sqlx::sqlite::SqliteRow) -> Result<McpServerConfig> {
    let enabled: i64 = r.try_get("enabled")?;
    Ok(McpServerConfig {
        id: parse_uuid(r.try_get::<String, _>("id")?)?,
        name: r.try_get("name")?,
        transport: from_json::<McpTransport>(&r.try_get::<String, _>("transport")?)?,
        permission_mode: from_json::<PermissionMode>(&r.try_get::<String, _>("permission_mode")?)?,
        enabled: enabled != 0,
    })
}

// --- Providers --------------------------------------------------------------

/// CRUD for [`ProviderConfig`] rows.
///
/// The `api_key_ref` column stores ONLY the [`SecretRef`] handle string, never
/// key material (Section 7.2 / 9.1).
pub struct ProviderRepo<'a> {
    pool: &'a SqlitePool,
}

impl<'a> ProviderRepo<'a> {
    pub fn new(db: &'a Db) -> Self {
        Self { pool: db.pool() }
    }

    pub async fn insert(&self, p: &ProviderConfig) -> Result<()> {
        sqlx::query(
            "INSERT INTO providers (id, kind, base_url, api_key_ref, extra) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&p.id)
        .bind(to_json(&p.kind)?)
        .bind(&p.base_url)
        .bind(p.api_key_ref.as_ref().map(|r| r.handle().to_string()))
        .bind(to_json(&p.extra)?)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    pub async fn get(&self, id: &str) -> Result<Option<ProviderConfig>> {
        let row = sqlx::query("SELECT * FROM providers WHERE id = ?")
            .bind(id)
            .fetch_optional(self.pool)
            .await?;
        row.map(|r| row_to_provider(&r)).transpose()
    }

    pub async fn list(&self) -> Result<Vec<ProviderConfig>> {
        let rows = sqlx::query("SELECT * FROM providers ORDER BY id ASC")
            .fetch_all(self.pool)
            .await?;
        rows.iter().map(row_to_provider).collect()
    }

    pub async fn update(&self, p: &ProviderConfig) -> Result<()> {
        let res = sqlx::query(
            "UPDATE providers SET kind = ?, base_url = ?, api_key_ref = ?, extra = ? WHERE id = ?",
        )
        .bind(to_json(&p.kind)?)
        .bind(&p.base_url)
        .bind(p.api_key_ref.as_ref().map(|r| r.handle().to_string()))
        .bind(to_json(&p.extra)?)
        .bind(&p.id)
        .execute(self.pool)
        .await?;
        if res.rows_affected() == 0 {
            return Err(PersistenceError::NotFound(format!("provider {}", p.id)));
        }
        Ok(())
    }

    pub async fn delete(&self, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM providers WHERE id = ?")
            .bind(id)
            .execute(self.pool)
            .await?;
        Ok(())
    }
}

fn row_to_provider(r: &sqlx::sqlite::SqliteRow) -> Result<ProviderConfig> {
    let api_key_ref: Option<String> = r.try_get("api_key_ref")?;
    Ok(ProviderConfig {
        id: r.try_get("id")?,
        kind: from_json::<ProviderKind>(&r.try_get::<String, _>("kind")?)?,
        base_url: r.try_get("base_url")?,
        api_key_ref: api_key_ref.map(SecretRef::new),
        extra: from_json::<serde_json::Value>(&r.try_get::<String, _>("extra")?)?,
    })
}

// --- Shared helpers ---------------------------------------------------------

fn opt_to_json<T: serde::Serialize>(value: &Option<T>) -> Result<Option<String>> {
    match value {
        Some(v) => Ok(Some(to_json(v)?)),
        None => Ok(None),
    }
}

fn opt_from_json<T: serde::de::DeserializeOwned>(value: Option<String>) -> Result<Option<T>> {
    match value {
        Some(s) => Ok(Some(from_json(&s)?)),
        None => Ok(None),
    }
}

fn parse_uuid(s: String) -> Result<Uuid> {
    Uuid::parse_str(&s).map_err(|e| PersistenceError::NotFound(format!("invalid uuid '{s}': {e}")))
}

fn parse_dt(s: String) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| PersistenceError::NotFound(format!("invalid timestamp '{s}': {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_db() -> Db {
        Db::open_in_memory().await.unwrap()
    }

    fn sample_conversation() -> Conversation {
        let now = Utc::now();
        Conversation {
            id: Uuid::new_v4(),
            title: "First".to_string(),
            created_at: now,
            updated_at: now,
            persona_id: None,
            conversation_pref: None,
            privacy_tags: vec![PrivacyTag::LocalOnly],
            enabled_tool_servers: vec![],
        }
    }

    #[tokio::test]
    async fn conversation_crud_round_trip() {
        let db = test_db().await;
        let repo = ConversationRepo::new(&db);
        let mut conv = sample_conversation();

        repo.insert(&conv).await.unwrap();
        let got = repo.get(conv.id).await.unwrap().unwrap();
        assert_eq!(got.title, "First");
        assert_eq!(got.privacy_tags, vec![PrivacyTag::LocalOnly]);

        conv.title = "Renamed".to_string();
        conv.conversation_pref = Some(ManualRoute {
            provider_id: "openai".to_string(),
            model: "gpt-4o".to_string(),
        });
        repo.update(&conv).await.unwrap();

        let list = repo.list().await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].title, "Renamed");
        assert_eq!(list[0].conversation_pref.as_ref().unwrap().model, "gpt-4o");

        repo.delete(conv.id).await.unwrap();
        assert!(repo.get(conv.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn message_crud_round_trip() {
        let db = test_db().await;
        let conv = sample_conversation();
        ConversationRepo::new(&db).insert(&conv).await.unwrap();

        let repo = MessageRepo::new(&db);
        let mut msg = Message {
            id: Uuid::new_v4(),
            conversation_id: conv.id,
            role: Role::User,
            content: MessageContent::Text {
                text: "hello".to_string(),
            },
            created_at: Utc::now(),
            route: None,
            usage: None,
            status: MessageStatus::Complete,
        };
        repo.insert(&msg).await.unwrap();

        let got = repo.get(msg.id).await.unwrap().unwrap();
        assert!(matches!(got.status, MessageStatus::Complete));

        msg.usage = Some(TokenUsage {
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: 15,
        });
        repo.update(&msg).await.unwrap();

        let list = repo.list_for_conversation(conv.id).await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].usage.unwrap().total_tokens, 15);

        repo.delete(msg.id).await.unwrap();
        assert!(repo
            .list_for_conversation(conv.id)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn persona_crud_round_trip() {
        let db = test_db().await;
        let repo = PersonaRepo::new(&db);
        let mut persona = AgentPersona {
            id: Uuid::new_v4(),
            name: "Coder".to_string(),
            system_prompt: "You write code.".to_string(),
            default_route: None,
            routing_hint: Some(RoutingHint::PreferLocal),
            allowed_tool_servers: vec![Uuid::new_v4()],
            parameters: ModelParameters {
                temperature: Some(0.2),
                ..Default::default()
            },
        };
        repo.insert(&persona).await.unwrap();

        let got = repo.get(persona.id).await.unwrap().unwrap();
        assert_eq!(got.name, "Coder");
        assert_eq!(got.parameters.temperature, Some(0.2));

        persona.name = "Coder v2".to_string();
        repo.update(&persona).await.unwrap();
        assert_eq!(repo.list().await.unwrap()[0].name, "Coder v2");

        repo.delete(persona.id).await.unwrap();
        assert!(repo.get(persona.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn mcp_server_crud_round_trip() {
        let db = test_db().await;
        let repo = McpServerRepo::new(&db);
        let mut server = McpServerConfig {
            id: Uuid::new_v4(),
            name: "fs".to_string(),
            transport: McpTransport::Stdio {
                command: "mcp-fs".to_string(),
                args: vec!["--root".to_string(), "/tmp".to_string()],
                env: vec![],
            },
            permission_mode: PermissionMode::Ask,
            enabled: true,
        };
        repo.insert(&server).await.unwrap();

        let got = repo.get(server.id).await.unwrap().unwrap();
        assert!(got.enabled);
        assert!(matches!(got.permission_mode, PermissionMode::Ask));

        server.enabled = false;
        repo.update(&server).await.unwrap();
        assert!(!repo.get(server.id).await.unwrap().unwrap().enabled);

        repo.delete(server.id).await.unwrap();
        assert!(repo.get(server.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn provider_crud_stores_only_secret_ref_handle() {
        let db = test_db().await;
        let repo = ProviderRepo::new(&db);
        let mut provider = ProviderConfig {
            id: "openai".to_string(),
            kind: ProviderKind::OpenAI,
            base_url: None,
            api_key_ref: Some(SecretRef::new("openai-key")),
            extra: serde_json::json!({"org": "acme"}),
        };
        repo.insert(&provider).await.unwrap();

        let got = repo.get("openai").await.unwrap().unwrap();
        assert_eq!(got.api_key_ref, Some(SecretRef::new("openai-key")));

        // The raw api_key_ref column holds only the handle string, never a key.
        let raw: String = sqlx::query("SELECT api_key_ref FROM providers WHERE id = ?")
            .bind("openai")
            .fetch_one(db.pool())
            .await
            .unwrap()
            .try_get("api_key_ref")
            .unwrap();
        assert_eq!(raw, "openai-key");

        provider.base_url = Some("https://api.example.com".to_string());
        repo.update(&provider).await.unwrap();
        assert_eq!(
            repo.get("openai")
                .await
                .unwrap()
                .unwrap()
                .base_url
                .as_deref(),
            Some("https://api.example.com")
        );

        repo.delete("openai").await.unwrap();
        assert!(repo.get("openai").await.unwrap().is_none());
    }
}
