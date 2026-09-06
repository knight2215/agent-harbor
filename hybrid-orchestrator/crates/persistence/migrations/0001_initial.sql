-- Initial schema (architecture.md Section 7.2).
--
-- SQLite via sqlx. Complex/structured fields (privacy_tags, enabled_tool_servers,
-- message content, route, usage, transport, model parameters, provider extras)
-- are stored as JSON TEXT columns serialized with serde_json. Secrets are NEVER
-- stored here: the providers table holds only the SecretRef handle string in
-- `api_key_ref` (Section 7.2 / 9.1).
--
-- Timestamps are RFC3339 TEXT (chrono DateTime<Utc> round-trips losslessly).
-- Uuids are stored as TEXT (their hyphenated string form).

CREATE TABLE conversations (
    id                   TEXT PRIMARY KEY NOT NULL,
    title                TEXT NOT NULL,
    created_at           TEXT NOT NULL,
    updated_at           TEXT NOT NULL,
    persona_id           TEXT,
    conversation_pref    TEXT,             -- JSON: Option<ManualRoute>
    privacy_tags         TEXT NOT NULL,    -- JSON: Vec<PrivacyTag>
    enabled_tool_servers TEXT NOT NULL     -- JSON: Vec<Uuid>
);

CREATE TABLE messages (
    id              TEXT PRIMARY KEY NOT NULL,
    conversation_id TEXT NOT NULL,
    role            TEXT NOT NULL,         -- JSON: Role
    content         TEXT NOT NULL,         -- JSON: MessageContent
    created_at      TEXT NOT NULL,
    route           TEXT,                  -- JSON: Option<RouteMetadata>
    usage           TEXT,                  -- JSON: Option<TokenUsage>
    status          TEXT NOT NULL,         -- JSON: MessageStatus
    FOREIGN KEY (conversation_id) REFERENCES conversations (id) ON DELETE CASCADE
);

CREATE INDEX idx_messages_conversation_id ON messages (conversation_id);

CREATE TABLE agent_personas (
    id                   TEXT PRIMARY KEY NOT NULL,
    name                 TEXT NOT NULL,
    system_prompt        TEXT NOT NULL,
    default_route        TEXT,             -- JSON: Option<ManualRoute>
    routing_hint         TEXT,             -- JSON: Option<RoutingHint>
    allowed_tool_servers TEXT NOT NULL,    -- JSON: Vec<Uuid>
    parameters           TEXT NOT NULL     -- JSON: ModelParameters
);

CREATE TABLE mcp_servers (
    id              TEXT PRIMARY KEY NOT NULL,
    name            TEXT NOT NULL,
    transport       TEXT NOT NULL,         -- JSON: McpTransport
    permission_mode TEXT NOT NULL,         -- JSON: PermissionMode
    enabled         INTEGER NOT NULL       -- 0/1 boolean
);

CREATE TABLE providers (
    id          TEXT PRIMARY KEY NOT NULL,
    kind        TEXT NOT NULL,             -- JSON: ProviderKind
    base_url    TEXT,
    api_key_ref TEXT,                      -- SecretRef handle string ONLY, never a key
    extra       TEXT NOT NULL              -- JSON: serde_json::Value
);

-- Single-row key/value config store. `schema_version` (Section 10.4) is kept in
-- a dedicated column so migrations can reason about it; the remaining config is
-- a JSON blob with forward-compatible load (unknown newer fields preserved).
CREATE TABLE app_config (
    id             INTEGER PRIMARY KEY CHECK (id = 1),
    schema_version INTEGER NOT NULL,
    data           TEXT NOT NULL           -- JSON: forward-compatible config blob
);
