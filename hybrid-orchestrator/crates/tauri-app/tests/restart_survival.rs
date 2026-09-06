//! Restart-survival acceptance (FEAT-003).
//!
//! `tauri dev` cannot run in the offline sandbox (no Tauri CLI), so the
//! "a conversation and a persona survive restart" acceptance is verified here
//! as an automated test that CI runs via
//! `cargo test --manifest-path crates/tauri-app/Cargo.toml`.
//!
//! The test opens the real `AppState` initialization path against a temp
//! SQLite FILE (not `:memory:`, whose data would vanish with the pool), writes
//! a conversation and a persona, DROPS the manager/pool (simulating app exit),
//! then reopens a FRESH `Db`/`SessionManager` against the SAME file and asserts
//! both rows are still present. This is a genuine round-trip through the
//! persistence layer, not a static assertion.

use std::sync::Arc;

use orchestrator_core::{AgentPersona, ConversationInit, ModelParameters, SessionManager};
use persistence::Db;
use secrets::InMemorySecretStore;
use uuid::Uuid;

/// Build an `AppState` around a `Db` opened at `db_path`, using an in-memory
/// secret store so the test needs no OS keychain (headless CI has no Secret
/// Service).
async fn state_at(db_path: &std::path::Path) -> tauri_app::state::AppState {
    let db = Db::open(db_path).await.expect("open db + run migrations");
    let (state, _rx) = tauri_app::state::AppState::new(
        SessionManager::new(db),
        Arc::new(InMemorySecretStore::new()),
    );
    state
}

#[tokio::test]
async fn conversation_and_persona_survive_restart() {
    let dir = tempfile::tempdir().expect("temp dir");
    let db_path = dir.path().join("restart-test.sqlite3");

    let persona_id = Uuid::new_v4();
    let conversation_id;

    // --- First "run": create a conversation and a persona, then drop everything.
    {
        let state = state_at(&db_path).await;

        let conv = state
            .session_manager
            .create_conversation(ConversationInit {
                title: Some("Persisted chat".to_string()),
                ..Default::default()
            })
            .await
            .expect("create conversation");
        conversation_id = conv.id;

        let persona = AgentPersona {
            id: persona_id,
            name: "Persisted persona".to_string(),
            system_prompt: "You persist.".to_string(),
            default_route: None,
            routing_hint: None,
            allowed_tool_servers: vec![],
            parameters: ModelParameters::default(),
        };
        state
            .session_manager
            .personas()
            .insert(&persona)
            .await
            .expect("insert persona");

        // `state` (and the SessionManager + SqlitePool it owns) is dropped here,
        // closing the connection just like an app shutdown.
    }

    // --- Second "run": reopen a FRESH Db/SessionManager against the SAME file.
    {
        let state = state_at(&db_path).await;

        let conversations = state
            .session_manager
            .list_conversations()
            .await
            .expect("list conversations");
        assert_eq!(
            conversations.len(),
            1,
            "the conversation must survive restart"
        );
        assert_eq!(conversations[0].id, conversation_id);
        assert_eq!(conversations[0].title, "Persisted chat");

        let personas = state
            .session_manager
            .personas()
            .list()
            .await
            .expect("list personas");
        assert_eq!(personas.len(), 1, "the persona must survive restart");
        assert_eq!(personas[0].id, persona_id);
        assert_eq!(personas[0].name, "Persisted persona");
    }
}
