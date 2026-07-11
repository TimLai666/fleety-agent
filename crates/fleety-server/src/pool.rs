//! A group of provider accounts/endpoints fronted as one [`ModelProvider`].
//!
//! A [`PoolProvider`] holds several member providers of the same model (multiple
//! Codex accounts, several OpenAI-compatible endpoints) and dispatches each call
//! across them. On *any* member error — the member already exhausted its own
//! internal retries (see `agent-core` resilient model calls) — it moves to the
//! next member, returning an error only once all members have failed. The pool
//! itself never retries a single member; it only fails over.
//!
//! - `round_robin` advances the starting member per call to spread load (so N
//!   accounts share the quota).
//! - `failover` always starts at the first member and only advances on failure.
//!
//! The attempt order is a pure function of `(start, member count)`, so the
//! dispatch sequence is exhaustively testable without any HTTP.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use agent_core::model::{Effort, ModelCapabilities};
use agent_core::{CoreError, Message, ModelProvider, ModelResponse, Result, ToolSpec};
use fleety_tools::providers_config::Strategy;

/// The order in which members are tried, starting at `start` (wrapped into
/// range) and cycling once through all `len` members. Empty when `len == 0`.
pub fn attempt_order(start: usize, len: usize) -> Vec<usize> {
    if len == 0 {
        return Vec::new();
    }
    let s = start % len;
    (0..len).map(|i| (s + i) % len).collect()
}

/// Several member providers fronted as one, dispatched by [`Strategy`].
pub struct PoolProvider {
    members: Vec<Arc<dyn ModelProvider>>,
    strategy: Strategy,
    next: AtomicUsize,
}

impl PoolProvider {
    /// Build a pool over `members` with the given dispatch `strategy`.
    pub fn new(members: Vec<Arc<dyn ModelProvider>>, strategy: Strategy) -> Self {
        Self {
            members,
            strategy,
            next: AtomicUsize::new(0),
        }
    }

    /// The starting member index for this call: round-robin advances a shared
    /// counter (spreading load); failover always starts at the first member.
    fn start(&self) -> usize {
        match self.strategy {
            Strategy::RoundRobin => self.next.fetch_add(1, Ordering::Relaxed),
            // Single (exactly one member) and failover both start at the first.
            Strategy::Single | Strategy::Failover => 0,
        }
    }

    fn empty_err() -> CoreError {
        CoreError::Message("provider pool has no members".to_string())
    }
}

#[async_trait::async_trait]
impl ModelProvider for PoolProvider {
    async fn complete(&self, messages: &[Message], tools: &[ToolSpec]) -> Result<ModelResponse> {
        let len = self.members.len();
        let mut last: Option<CoreError> = None;
        for idx in attempt_order(self.start(), len) {
            match self.members[idx].complete(messages, tools).await {
                Ok(r) => return Ok(r),
                Err(e) => last = Some(e),
            }
        }
        Err(last.unwrap_or_else(Self::empty_err))
    }

    async fn complete_streaming(
        &self,
        messages: &[Message],
        tools: &[ToolSpec],
        on_delta: &mut (dyn for<'a> FnMut(&'a str) + Send),
    ) -> Result<ModelResponse> {
        let len = self.members.len();
        let mut last: Option<CoreError> = None;
        for idx in attempt_order(self.start(), len) {
            // Only fail over *before* the first delta: once a member has streamed
            // output, switching members would re-emit. (Members retry transient
            // errors internally before their first delta, so this is rare.)
            let mut emitted = false;
            let res = {
                let mut wrapper = |s: &str| {
                    emitted = true;
                    on_delta(s);
                };
                self.members[idx]
                    .complete_streaming(messages, tools, &mut wrapper)
                    .await
            };
            match res {
                Ok(r) => return Ok(r),
                Err(e) => {
                    last = Some(e);
                    if emitted {
                        break;
                    }
                }
            }
        }
        Err(last.unwrap_or_else(Self::empty_err))
    }

    fn capabilities(&self) -> ModelCapabilities {
        // Union across members (design M2): a modality is supported if ANY member
        // supports it, so a mixed pool never blocks an attachment some member
        // could handle — the routed member degrades what it cannot take. Never
        // the first member's caps, nor the intersection.
        self.members
            .iter()
            .fold(ModelCapabilities::TEXT_ONLY, |acc, m| {
                let c = m.capabilities();
                ModelCapabilities {
                    image: acc.image || c.image,
                    audio: acc.audio || c.audio,
                    pdf: acc.pdf || c.pdf,
                }
            })
    }

    fn with_effort(&self, effort: Option<Effort>) -> Option<Arc<dyn ModelProvider>> {
        let members = self
            .members
            .iter()
            .map(|m| m.with_effort(effort).unwrap_or_else(|| Arc::clone(m)))
            .collect();
        Some(Arc::new(PoolProvider::new(members, self.strategy)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attempt_order_table() {
        // Spec example table.
        assert_eq!(attempt_order(0, 3), vec![0, 1, 2]); // round_robin call 1
        assert_eq!(attempt_order(1, 3), vec![1, 2, 0]); // round_robin call 2
                                                        // failover (start 0) is the same as round_robin call 1.
        assert_eq!(attempt_order(0, 3), vec![0, 1, 2]);
        // Boundaries: single member, start past the end, empty.
        assert_eq!(attempt_order(7, 1), vec![0]);
        assert_eq!(attempt_order(5, 3), vec![2, 0, 1]);
        assert_eq!(attempt_order(0, 0), Vec::<usize>::new());
    }

    /// A member that either always succeeds (echoing `tag`) or always errors,
    /// with configurable capabilities.
    struct Stub {
        ok: bool,
        tag: &'static str,
        caps: ModelCapabilities,
    }

    impl Stub {
        fn ok(tag: &'static str) -> Arc<dyn ModelProvider> {
            Arc::new(Stub {
                ok: true,
                tag,
                caps: ModelCapabilities::ALL,
            })
        }
        fn err() -> Arc<dyn ModelProvider> {
            Arc::new(Stub {
                ok: false,
                tag: "",
                caps: ModelCapabilities::ALL,
            })
        }
    }

    #[async_trait::async_trait]
    impl ModelProvider for Stub {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolSpec],
        ) -> Result<ModelResponse> {
            if self.ok {
                Ok(ModelResponse {
                    message: Message::assistant(self.tag),
                })
            } else {
                Err(CoreError::Message("boom".to_string()))
            }
        }
        fn capabilities(&self) -> ModelCapabilities {
            self.caps
        }
    }

    fn content(r: &ModelResponse) -> Option<&str> {
        r.message.content.as_deref()
    }

    #[tokio::test]
    async fn failover_to_next_member_on_error() {
        let pool = PoolProvider::new(vec![Stub::err(), Stub::ok("b")], Strategy::Failover);
        let r = pool
            .complete(&[], &[])
            .await
            .expect("second member succeeds");
        assert_eq!(content(&r), Some("b"));
    }

    #[tokio::test]
    async fn all_members_fail_returns_last_error() {
        let pool = PoolProvider::new(vec![Stub::err(), Stub::err()], Strategy::Failover);
        assert!(pool.complete(&[], &[]).await.is_err());
    }

    #[tokio::test]
    async fn round_robin_advances_start() {
        let pool = PoolProvider::new(vec![Stub::ok("a"), Stub::ok("b")], Strategy::RoundRobin);
        // start 0 → a, start 1 → b, start 2%2=0 → a.
        let r1 = pool.complete(&[], &[]).await.unwrap();
        let r2 = pool.complete(&[], &[]).await.unwrap();
        let r3 = pool.complete(&[], &[]).await.unwrap();
        assert_eq!(
            (content(&r1), content(&r2), content(&r3)),
            (Some("a"), Some("b"), Some("a"))
        );
    }

    #[tokio::test]
    async fn capabilities_are_union_across_members() {
        // Spec example: a text-only member + an image-capable member → the pool
        // reports image supported (union), not suppressed by the first being
        // text-only.
        let text_only = Arc::new(Stub {
            ok: true,
            tag: "a",
            caps: ModelCapabilities::TEXT_ONLY,
        });
        let image_capable = Arc::new(Stub {
            ok: true,
            tag: "b",
            caps: ModelCapabilities {
                image: true,
                audio: false,
                pdf: false,
            },
        });
        let pool = PoolProvider::new(vec![text_only, image_capable], Strategy::Failover);
        let caps = pool.capabilities();
        assert!(
            caps.image,
            "union: image supported because a member supports it"
        );
        assert!(
            !caps.audio,
            "union: audio unsupported (no member supports it)"
        );
    }
}
