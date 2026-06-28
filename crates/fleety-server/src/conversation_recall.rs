//! Conversation recall: let the agent search the **acting user's** past
//! conversations, with time order. Scoped per user (privacy-isolation made
//! conversations user-primary), so a turn only ever recalls its own user's
//! history.
//!
//! v1 ships keyword search + a conversation listing (both exact, no model). The
//! semantic tool degrades to keyword with a note; embedding-ranked recall (a
//! per-user vector index reusing the wiki's embedding/sqlite-vec layer) is a
//! follow-up. Results carry `conversation_id`, `seq`, `ts_secs`, `role`, and a
//! snippet so the agent knows what came before what.

use std::sync::Arc;

use agent_core::{Result, RiskLevel, Role, Tool, ToolRegistry, ToolSpec};
use async_trait::async_trait;
use serde::Serialize;
use serde_json::{json, Value};

use crate::storage::Storage;

#[derive(Debug, Clone, Serialize)]
pub struct RecallHit {
    pub conversation_id: String,
    pub seq: u64,
    pub ts_secs: u64,
    pub role: String,
    pub snippet: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f32>,
}

fn role_str(role: &Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
}

fn snippet(text: &str) -> String {
    const MAX: usize = 200;
    let t = text.trim();
    if t.chars().count() <= MAX {
        t.to_string()
    } else {
        let kept: String = t.chars().take(MAX).collect();
        format!("{kept}…")
    }
}

/// Keyword search over a user's conversations, newest-first.
fn keyword_search(storage: &Storage, user: &str, query: &str, limit: usize) -> Vec<RecallHit> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    let mut hits = Vec::new();
    for conv in storage.user_conversation_ids(user) {
        for ev in storage.load_user_conversation(user, &conv) {
            if let Some(content) = &ev.message.content {
                if content.to_lowercase().contains(&needle) {
                    hits.push(RecallHit {
                        conversation_id: conv.clone(),
                        seq: ev.seq,
                        ts_secs: ev.ts_secs,
                        role: role_str(&ev.message.role).to_string(),
                        snippet: snippet(content),
                        score: None,
                    });
                }
            }
        }
    }
    // Newest-first by time, then sequence.
    hits.sort_by(|a, b| b.ts_secs.cmp(&a.ts_secs).then(b.seq.cmp(&a.seq)));
    hits.truncate(limit);
    hits
}

fn limit_arg(args: &Value) -> usize {
    args.get("limit")
        .and_then(Value::as_u64)
        .map(|n| n.clamp(1, 50) as usize)
        .unwrap_or(10)
}

/// Register the recall tools, scoped to `user` (the acting user). A Guest (None)
/// gets tools that return nothing — no access to any real user's history.
pub fn register(tools: &mut ToolRegistry, storage: Arc<Storage>, user: Option<String>) {
    tools.register(Box::new(ConversationSearch {
        storage: Arc::clone(&storage),
        user: user.clone(),
    }));
    tools.register(Box::new(ConversationSemanticSearch {
        storage: Arc::clone(&storage),
        user: user.clone(),
    }));
    tools.register(Box::new(ConversationList { storage, user }));
}

struct ConversationSearch {
    storage: Arc<Storage>,
    user: Option<String>,
}

#[async_trait]
impl Tool for ConversationSearch {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "conversation_search".to_string(),
            description:
                "Keyword-search your past conversations (this user's only), newest-first. \
                Returns matches with conversation_id, seq, ts_secs (UTC epoch), role, and a \
                snippet. Use it to recall what was discussed and when."
                    .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Text to search for." },
                    "limit": { "type": "integer", "description": "Max results (1-50, default 10)." }
                },
                "required": ["query"]
            }),
            risk: RiskLevel::Read,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let query = args.get("query").and_then(Value::as_str).unwrap_or("");
        let hits = match &self.user {
            Some(u) => keyword_search(&self.storage, u, query, limit_arg(&args)),
            None => Vec::new(),
        };
        Ok(json!({ "hits": hits }))
    }
}

struct ConversationSemanticSearch {
    storage: Arc<Storage>,
    user: Option<String>,
}

#[async_trait]
impl Tool for ConversationSemanticSearch {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "conversation_semantic_search".to_string(),
            description: "Semantic search of your past conversations (this user's only). v1 falls \
                back to keyword matching (embedding-ranked recall is a planned follow-up); results \
                carry timestamps so you know their order."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "What to look for (meaning-based)." },
                    "limit": { "type": "integer", "description": "Max results (1-50, default 10)." }
                },
                "required": ["query"]
            }),
            risk: RiskLevel::Read,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let query = args.get("query").and_then(Value::as_str).unwrap_or("");
        let hits = match &self.user {
            Some(u) => keyword_search(&self.storage, u, query, limit_arg(&args)),
            None => Vec::new(),
        };
        Ok(json!({
            "hits": hits,
            "note": "semantic ranking unavailable in this build; returned keyword matches"
        }))
    }
}

struct ConversationList {
    storage: Arc<Storage>,
    user: Option<String>,
}

#[async_trait]
impl Tool for ConversationList {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "conversation_list".to_string(),
            description:
                "List your past conversations (this user's only) with their last-activity \
                time (ts_secs, UTC epoch) and event count, most-recent first."
                    .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "limit": { "type": "integer", "description": "Max conversations (1-50, default 20)." }
                }
            }),
            risk: RiskLevel::Read,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let Some(user) = &self.user else {
            return Ok(json!({ "conversations": [] }));
        };
        let limit = args
            .get("limit")
            .and_then(Value::as_u64)
            .map(|n| n.clamp(1, 50) as usize)
            .unwrap_or(20);
        let mut convs: Vec<Value> = self
            .storage
            .user_conversation_ids(user)
            .into_iter()
            .map(|conv| {
                let events = self.storage.load_user_conversation(user, &conv);
                let last_ts = events.iter().map(|e| e.ts_secs).max().unwrap_or(0);
                json!({
                    "conversation_id": conv,
                    "last_ts_secs": last_ts,
                    "events": events.len(),
                })
            })
            .collect();
        convs.sort_by(|a, b| {
            b.get("last_ts_secs")
                .and_then(Value::as_u64)
                .cmp(&a.get("last_ts_secs").and_then(Value::as_u64))
        });
        convs.truncate(limit);
        Ok(json!({ "conversations": convs }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::Message;

    fn temp_storage() -> (Storage, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("fleety-recall-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        (Storage::new(dir.clone()), dir)
    }

    #[test]
    fn keyword_search_scopes_per_user_newest_first() {
        let (storage, home) = temp_storage();
        let alice = crate::identity::ActingUser::User("alice".into());
        storage.register_conversation_owner("c1", &alice).unwrap();
        storage
            .append("dev", "c1", &Message::user("deploy the server tonight"))
            .unwrap();
        storage.register_conversation_owner("c2", &alice).unwrap();
        storage
            .append("dev", "c2", &Message::assistant("the deploy finished"))
            .unwrap();
        // bob's conversation must never appear in alice's recall.
        let bob = crate::identity::ActingUser::User("bob".into());
        storage.register_conversation_owner("b1", &bob).unwrap();
        storage
            .append("dev", "b1", &Message::user("deploy secret"))
            .unwrap();

        let hits = keyword_search(&storage, "alice", "deploy", 10);
        assert_eq!(hits.len(), 2, "both of alice's matches, not bob's");
        assert!(hits
            .iter()
            .all(|h| h.conversation_id == "c1" || h.conversation_id == "c2"));
        // newest-first: c2 (appended later) has a >= ts and should sort first or tie.
        assert!(hits[0].ts_secs >= hits[1].ts_secs);
        // Guest / other-scope returns nothing for alice's data.
        assert!(keyword_search(&storage, "bob", "tonight", 10).is_empty());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[tokio::test]
    async fn guest_recall_is_empty() {
        let (storage, home) = temp_storage();
        let storage = Arc::new(storage);
        let mut reg = ToolRegistry::new();
        register(&mut reg, Arc::clone(&storage), None);
        let out = reg
            .call("conversation_search", json!({ "query": "anything" }))
            .await
            .unwrap();
        assert_eq!(out["hits"].as_array().map(|a| a.len()), Some(0));
        // semantic degrades with a note, never errors.
        let sem = reg
            .call("conversation_semantic_search", json!({ "query": "x" }))
            .await
            .unwrap();
        assert!(sem["note"].is_string());
        let _ = std::fs::remove_dir_all(&home);
    }
}
