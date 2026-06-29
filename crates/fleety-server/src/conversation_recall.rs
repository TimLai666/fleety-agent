//! Conversation recall: let the agent search the **acting user's** past
//! conversations, with time order. Scoped per user (privacy-isolation made
//! conversations user-primary), so a turn only ever recalls its own user's
//! history.
//!
//! Tools: keyword search + a conversation listing (exact, no model), and an
//! embedding-ranked semantic search backed by a per-user vector index
//! (`conversation_embed`, reusing the wiki's embedding/sqlite-vec layer). Semantic
//! search degrades to keyword when embeddings are unavailable, and lazily
//! backfills pre-existing conversations on first use. Results carry
//! `conversation_id`, `seq`, `ts_secs`, `role`, and a snippet so the agent knows
//! what came before what.

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
    tools.register(Box::new(ConversationList {
        storage: Arc::clone(&storage),
        user: user.clone(),
    }));
    tools.register(Box::new(SubagentList { storage, user }));
}

struct SubagentList {
    storage: Arc<Storage>,
    user: Option<String>,
}

#[async_trait]
impl Tool for SubagentList {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "subagent_list".to_string(),
            description: "List the subagent child conversations a conversation spawned. Pass a \
                conversation_id; returns the child conversation ids (each readable via \
                conversation_search / conversation_semantic_search). Only your own conversations."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "conversation_id": { "type": "string", "description": "The parent conversation id." }
                },
                "required": ["conversation_id"]
            }),
            risk: RiskLevel::Read,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let Some(user) = self.user.clone() else {
            return Ok(json!({ "children": [] })); // guest
        };
        let conv = args
            .get("conversation_id")
            .and_then(Value::as_str)
            .unwrap_or("");
        if conv.is_empty() {
            return Ok(json!({ "children": [] }));
        }
        // Only over the acting user's accessible conversations (no cross-user leak).
        let acting = crate::identity::ActingUser::User(user);
        let allowed = self
            .storage
            .conversation_access(&acting, conv, &self.storage.grants())
            .is_allow();
        let children = if allowed {
            self.storage.subagent_children(conv)
        } else {
            Vec::new()
        };
        Ok(json!({ "children": children }))
    }
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
            description: "Semantic (embedding-ranked) search of your past conversations (this \
                user's only): finds messages by meaning, ranked by similarity, newest-first on \
                ties; results carry timestamps so you know their order. Falls back to keyword \
                matching when embeddings are unavailable."
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
        let query = args
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let limit = limit_arg(&args);
        // Guest: no identified user → no access to anyone's history.
        let Some(user) = self.user.clone() else {
            return Ok(json!({ "hits": [] }));
        };
        // Embedding-ranked when available; the model + sqlite are blocking.
        if crate::embed::enabled() && !query.trim().is_empty() {
            let path = self.storage.conversation_index_path(&user);
            let cache = self.storage.models_dir();
            let q = query.clone();
            let storage = Arc::clone(&self.storage);
            let user_b = user.clone();
            let result = tokio::task::spawn_blocking(move || {
                // Lazy backfill: conversations that predate the index aren't covered
                // by the off-turn incremental indexer, so on first search (empty
                // index) index all of this user's conversations once. index_messages
                // skips anything already indexed, so this is one-time.
                if crate::conversation_embed::is_empty(&path) {
                    for conv in storage.user_conversation_ids(&user_b) {
                        let msgs: Vec<crate::conversation_embed::IndexMsg> = storage
                            .load_user_conversation(&user_b, &conv)
                            .into_iter()
                            .filter_map(|e| {
                                let content = e.message.content?;
                                Some((
                                    e.seq,
                                    e.ts_secs,
                                    role_str(&e.message.role).to_string(),
                                    content,
                                ))
                            })
                            .collect();
                        let _ =
                            crate::conversation_embed::index_messages(&path, &cache, &conv, &msgs);
                    }
                }
                crate::conversation_embed::search(&path, &cache, &q, limit)
            })
            .await;
            if let Ok(Ok(chunks)) = result {
                if !chunks.is_empty() {
                    let hits: Vec<RecallHit> = chunks
                        .into_iter()
                        .map(|c| RecallHit {
                            conversation_id: c.conversation_id,
                            seq: c.seq,
                            ts_secs: c.ts_secs,
                            role: c.role,
                            snippet: c.snippet,
                            score: Some(c.score),
                        })
                        .collect();
                    return Ok(json!({ "hits": hits, "ranking": "embedding" }));
                }
            }
            // disabled mid-flight / empty / error → fall through to keyword.
        }
        let hits = keyword_search(&self.storage, &user, &query, limit);
        Ok(json!({ "hits": hits, "ranking": "keyword" }))
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
                // Metadata only — avoid deserializing every message in each file.
                let (count, last_ts) = self.storage.conversation_summary(user, &conv);
                json!({
                    "conversation_id": conv,
                    "last_ts_secs": last_ts,
                    "events": count,
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
    async fn subagent_list_scopes_to_owner() {
        let (storage, home) = temp_storage();
        let alice = crate::identity::ActingUser::User("alice".into());
        storage.register_conversation_owner("c1", &alice).unwrap();
        storage.link_subagent("c1", "sub-1").unwrap();
        storage.link_subagent("c1", "sub-2").unwrap();
        let storage = Arc::new(storage);

        // Owner sees the children.
        let tool = SubagentList {
            storage: Arc::clone(&storage),
            user: Some("alice".into()),
        };
        let out = tool.call(json!({ "conversation_id": "c1" })).await.unwrap();
        assert_eq!(out["children"].as_array().map(|a| a.len()), Some(2));
        // Another user gets nothing (no leak).
        let bob = SubagentList {
            storage: Arc::clone(&storage),
            user: Some("bob".into()),
        };
        let out = bob.call(json!({ "conversation_id": "c1" })).await.unwrap();
        assert_eq!(out["children"].as_array().map(|a| a.len()), Some(0));
        // Guest gets nothing.
        let guest = SubagentList {
            storage,
            user: None,
        };
        let out = guest.call(json!({ "conversation_id": "c1" })).await.unwrap();
        assert_eq!(out["children"].as_array().map(|a| a.len()), Some(0));
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
        // semantic also returns nothing for a guest, never errors.
        let sem = reg
            .call("conversation_semantic_search", json!({ "query": "x" }))
            .await
            .unwrap();
        assert_eq!(sem["hits"].as_array().map(|a| a.len()), Some(0));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn semantic_falls_back_to_keyword_when_embeddings_off() {
        std::env::set_var("FLEETY_WIKI_EMBED", "0");
        let (storage, home) = temp_storage();
        let alice = crate::identity::ActingUser::User("alice".into());
        storage.register_conversation_owner("c1", &alice).unwrap();
        storage
            .append("dev", "c1", &Message::user("deploy the server tonight"))
            .unwrap();
        let storage = Arc::new(storage);
        let mut reg = ToolRegistry::new();
        register(&mut reg, Arc::clone(&storage), Some("alice".into()));
        let out = reg
            .call("conversation_semantic_search", json!({ "query": "deploy" }))
            .await
            .unwrap();
        // Embeddings off → keyword fallback, never an error, never worse than before.
        assert_eq!(out["ranking"], "keyword");
        assert_eq!(out["hits"].as_array().map(|a| a.len()), Some(1));
        std::env::remove_var("FLEETY_WIKI_EMBED");
        let _ = std::fs::remove_dir_all(&home);
    }
}
