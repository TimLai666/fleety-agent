//! Dynamic workflow runtime.
//!
//! Runs a model-written JavaScript workflow script on an embedded boa engine and
//! binds its `agent()` / `parallel()` / `pipeline()` / `phase()` / `log()`
//! globals to a shared [`SubagentManager`]. Like Claude Code's Workflow, but
//! internal: `agent()` runs one of *our own* subagents, not an external CLI.
//!
//! The script body runs inside an async wrapper, so it may use top-level
//! `await`, and returns its result with `return`. Each `agent({prompt, ...})`
//! runs one foreground subagent (a leaf — it has no orchestration or workflow
//! tools, so the one-level nesting cap holds) and resolves to its output.
//!
//! boa's `Context` is `!Send` and single-threaded, so the shared manager is
//! reached through a thread-local set for the duration of a run. Multi-threaded
//! callers use [`run_isolated`], which runs the whole script on a dedicated
//! thread with its own current-thread runtime and bridges the result back.

#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used, clippy::expect_used)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use std::cell::RefCell;
use std::sync::Arc;

use boa_engine::context::ContextBuilder;
use boa_engine::job::JobExecutor;
use boa_engine::object::builtins::JsPromise;
use boa_engine::{
    js_string, Context, JsArgs, JsError, JsNativeError, JsObject, JsResult, JsValue,
    NativeFunction, Source,
};

use agent_core::{
    CoreError, Result, RiskLevel, SpawnRequest, SubagentManager, SubagentMode, Tool, ToolRegistry,
    ToolSpec,
};
use serde_json::{json, Value};

thread_local! {
    /// The subagent manager an `agent()` call delegates to, for the duration of
    /// one [`run_script`] on this thread.
    static MANAGER: RefCell<Option<Arc<SubagentManager>>> = const { RefCell::new(None) };
    /// `log()` lines collected during a run.
    static LOGS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    /// `phase()` markers collected during a run.
    static PHASES: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// JS prelude evaluated before the user's script: `parallel` and `pipeline` on
/// top of the engine's native `Promise.all` and the `agent`/`log`/`phase` natives.
const PRELUDE: &str = r#"
globalThis.parallel = function (thunks) {
    return Promise.all(thunks.map((t) => t()));
};
globalThis.pipeline = function (items, ...stages) {
    return Promise.all(items.map(async (item) => {
        let value = item;
        for (const stage of stages) {
            value = await stage(value, item);
        }
        return value;
    }));
};
"#;

fn opt_string(obj: &JsObject, key: &str, ctx: &mut Context) -> Option<String> {
    obj.get(js_string!(key), ctx)
        .ok()
        .filter(|v| !v.is_undefined() && !v.is_null())
        .and_then(|v| v.to_string(ctx).ok())
        .map(|s| s.to_std_string_escaped())
}

fn parse_request(opts: &JsValue, ctx: &mut Context) -> JsResult<SpawnRequest> {
    let obj = opts
        .as_object()
        .ok_or_else(|| js_err("agent(opts): opts must be an object with at least { prompt }"))?;
    let prompt = opt_string(&obj, "prompt", ctx)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| js_err("agent(opts): a non-empty `prompt` is required"))?;
    let mode = match opt_string(&obj, "mode", ctx).as_deref() {
        Some("fork") => SubagentMode::Fork,
        _ => SubagentMode::Spawn,
    };
    let tier = opt_string(&obj, "model", ctx).unwrap_or_else(|| "main".to_string());
    let effort = opt_string(&obj, "effort", ctx)
        .as_deref()
        .and_then(agent_core::model::Effort::parse);
    let isolation = opt_string(&obj, "isolation", ctx).unwrap_or_else(|| "none".to_string());
    let name = opt_string(&obj, "name", ctx);
    let allowed_tools = obj
        .get(js_string!("allowed_tools"), ctx)
        .ok()
        .and_then(|v| v.as_object().filter(|o| o.is_array()))
        .map(|arr| collect_string_array(&arr, ctx))
        .unwrap_or_default();
    Ok(SpawnRequest {
        mode,
        tier,
        effort,
        prompt,
        allowed_tools,
        isolation,
        name,
        background: false,
    })
}

fn collect_string_array(arr: &JsObject, ctx: &mut Context) -> Vec<String> {
    let len = arr
        .get(js_string!("length"), ctx)
        .ok()
        .and_then(|v| v.to_u32(ctx).ok())
        .unwrap_or(0);
    (0..len)
        .filter_map(|i| {
            arr.get(i, ctx)
                .ok()
                .and_then(|v| v.to_string(ctx).ok())
                .map(|s| s.to_std_string_escaped())
        })
        .collect()
}

fn js_err(msg: impl Into<String>) -> JsError {
    JsNativeError::error().with_message(msg.into()).into()
}

/// Native async `agent(opts)`: run one foreground subagent and resolve to its
/// output string.
async fn agent_native(
    _this: &JsValue,
    args: &[JsValue],
    context: &RefCell<&mut Context>,
) -> JsResult<JsValue> {
    let req = {
        let mut ctx = context.borrow_mut();
        parse_request(args.get_or_undefined(0), &mut ctx)?
    };
    let manager = MANAGER
        .with(|m| m.borrow().clone())
        .ok_or_else(|| js_err("workflow runtime: no subagent manager bound"))?;
    let result = manager
        .spawn(req)
        .await
        .map_err(|e| js_err(format!("subagent failed: {e}")))?;
    let output = result
        .get("output")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    // A foreground subagent that ran but failed comes back as `Ok` with
    // `state:"failed"`. Reject the promise so the script's try/catch and
    // `Promise.all` short-circuit see the failure instead of a bogus output.
    if result.get("state").and_then(Value::as_str) == Some("failed") {
        return Err(js_err(format!("subagent failed: {output}")));
    }
    Ok(JsValue::from(js_string!(output)))
}

/// Native `log(msg)`.
fn log_native(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let msg = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    LOGS.with(|l| l.borrow_mut().push(msg));
    Ok(JsValue::undefined())
}

/// Native `phase(name)`.
fn phase_native(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let name = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    PHASES.with(|p| p.borrow_mut().push(name));
    Ok(JsValue::undefined())
}

/// Run a workflow `script` on the current thread, delegating `agent()` calls to
/// `manager`. Returns `{ result, logs, phases }`. Never panics: a script or
/// agent error becomes an actionable [`CoreError`].
///
/// boa is `!Send`, so the caller must already be on a single-threaded context
/// (a `#[tokio::test]`, or [`run_isolated`]'s dedicated thread).
pub async fn run_script(manager: Arc<SubagentManager>, script: &str) -> Result<Value> {
    if script.trim().is_empty() {
        return Err(CoreError::Message("run_workflow: empty script".to_string()));
    }
    MANAGER.with(|m| *m.borrow_mut() = Some(manager));
    LOGS.with(|l| l.borrow_mut().clear());
    PHASES.with(|p| p.borrow_mut().clear());

    let outcome = run_inner(script).await;

    MANAGER.with(|m| *m.borrow_mut() = None);
    let logs: Vec<String> = LOGS.with(|l| l.borrow().clone());
    let phases: Vec<String> = PHASES.with(|p| p.borrow().clone());
    let result = outcome?;
    Ok(json!({ "result": result, "logs": logs, "phases": phases }))
}

async fn run_inner(script: &str) -> Result<Value> {
    let executor = std::rc::Rc::new(boa_engine::job::SimpleJobExecutor::new());
    let mut context = ContextBuilder::new()
        .job_executor(executor.clone())
        .build()
        .map_err(|e| CoreError::Message(format!("workflow engine init failed: {e}")))?;

    register_globals(&mut context).map_err(jerr)?;
    context
        .eval(Source::from_bytes(PRELUDE))
        .map_err(|e| CoreError::Message(format!("workflow prelude error: {}", fmt_js(e))))?;

    // Wrap the script so it can use top-level await and `return` its result.
    let wrapped = format!("(async () => {{\n{script}\n}})()");
    let promise_val = context
        .eval(Source::from_bytes(wrapped.as_bytes()))
        .map_err(|e| CoreError::Message(format!("workflow script error: {}", fmt_js(e))))?;

    std::rc::Rc::clone(&executor)
        .run_jobs_async(&RefCell::new(&mut context))
        .await
        .map_err(|e| CoreError::Message(format!("workflow run error: {}", fmt_js(e))))?;

    let promise = JsPromise::from_object(
        promise_val
            .as_object()
            .ok_or_else(|| CoreError::Message("workflow did not return a promise".to_string()))?
            .clone(),
    )
    .map_err(|_| CoreError::Message("workflow did not return a promise".to_string()))?;

    match promise.state() {
        boa_engine::builtins::promise::PromiseState::Fulfilled(v) => {
            json_stringify(&v, &mut context)
        }
        boa_engine::builtins::promise::PromiseState::Rejected(e) => Err(CoreError::Message(
            format!("workflow failed: {}", value_to_string(&e, &mut context)),
        )),
        boa_engine::builtins::promise::PromiseState::Pending => Err(CoreError::Message(
            "workflow did not complete (pending — an agent step never resolved)".to_string(),
        )),
    }
}

fn register_globals(context: &mut Context) -> JsResult<()> {
    context.register_global_callable(
        js_string!("agent"),
        1,
        NativeFunction::from_async_fn(agent_native),
    )?;
    context.register_global_callable(
        js_string!("log"),
        1,
        NativeFunction::from_fn_ptr(log_native),
    )?;
    context.register_global_callable(
        js_string!("phase"),
        1,
        NativeFunction::from_fn_ptr(phase_native),
    )?;
    Ok(())
}

fn jerr(e: JsError) -> CoreError {
    CoreError::Message(format!("workflow setup error: {e}"))
}

fn fmt_js(e: JsError) -> String {
    format!("{e}")
}

fn value_to_string(v: &JsValue, ctx: &mut Context) -> String {
    v.to_string(ctx)
        .map(|s| s.to_std_string_escaped())
        .unwrap_or_else(|_| "<unprintable>".to_string())
}

/// Serialize a JS value to JSON via the engine's `JSON.stringify`, then parse.
fn json_stringify(v: &JsValue, ctx: &mut Context) -> Result<Value> {
    if v.is_undefined() {
        return Ok(Value::Null);
    }
    let json = ctx
        .global_object()
        .get(js_string!("JSON"), ctx)
        .map_err(|e| CoreError::Message(format!("JSON missing: {}", fmt_js(e))))?;
    let stringify = json
        .as_object()
        .and_then(|o| o.get(js_string!("stringify"), ctx).ok())
        .ok_or_else(|| CoreError::Message("JSON.stringify missing".to_string()))?;
    let callable = stringify
        .as_callable()
        .ok_or_else(|| CoreError::Message("JSON.stringify not callable".to_string()))?;
    let s = callable
        .call(&JsValue::undefined(), std::slice::from_ref(v), ctx)
        .map_err(|e| CoreError::Message(format!("JSON.stringify failed: {}", fmt_js(e))))?;
    if s.is_undefined() {
        return Ok(Value::Null);
    }
    let s = s
        .to_string(ctx)
        .map_err(|e| CoreError::Message(format!("stringify result: {}", fmt_js(e))))?
        .to_std_string_escaped();
    serde_json::from_str(&s)
        .map_err(|e| CoreError::Message(format!("workflow result not JSON: {e}")))
}

/// Run a workflow on a dedicated thread (its own current-thread runtime), so a
/// multi-threaded caller can `.await` it despite boa being `!Send`.
pub async fn run_isolated(manager: Arc<SubagentManager>, script: String) -> Result<Value> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                let _ = tx.send(Err(CoreError::Message(format!(
                    "workflow runtime build failed: {e}"
                ))));
                return;
            }
        };
        let out = rt.block_on(run_script(manager, &script));
        let _ = tx.send(out);
    });
    rx.await
        .map_err(|_| CoreError::Message("workflow thread terminated unexpectedly".to_string()))?
}

/// Register the `run_workflow` tool on a top-level registry.
pub fn register_workflow(registry: &mut ToolRegistry, manager: Arc<SubagentManager>) {
    registry.register(Box::new(RunWorkflow(manager)));
}

struct RunWorkflow(Arc<SubagentManager>);

#[async_trait::async_trait]
impl Tool for RunWorkflow {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "run_workflow".to_string(),
            description: "Run a JavaScript workflow that deterministically orchestrates your own \
                subagents. The script body may use top-level `await` and ends by `return`-ing its \
                result. Globals: `agent({prompt, model?, mode?, isolation?, name?})` runs one \
                subagent and resolves to its output; `parallel(thunks)` runs thunks concurrently; \
                `pipeline(items, ...stages)` runs each item through the stages; `phase(name)` and \
                `log(msg)` record progress. Use this when the orchestration shape is dynamic and \
                worth pinning down as code."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "script": { "type": "string", "description": "The JS workflow script." }
                },
                "required": ["script"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let script = args
            .get("script")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                CoreError::Message("missing required string argument 'script'".to_string())
            })?
            .to_string();
        run_isolated(Arc::clone(&self.0), script).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{
        Message, MockProvider, ModelProvider, ModelResponse, Policy, SubagentHost, SubagentState,
    };

    struct EchoHost;

    #[async_trait::async_trait]
    impl SubagentHost for EchoHost {
        fn resolve_provider(&self, _tier: &str) -> Arc<dyn ModelProvider> {
            // Each subagent run pops one scripted reply.
            Arc::new(MockProvider::new(vec![ModelResponse::new(
                Message::assistant("ok"),
            )]))
        }
        async fn capture_context(&self) -> String {
            String::new()
        }
        async fn child_registry(&self, _workspace: Option<&str>) -> ToolRegistry {
            ToolRegistry::new()
        }
        async fn initial_messages(
            &self,
            _mode: SubagentMode,
            _context: &str,
            prompt: &str,
        ) -> Vec<Message> {
            // Echo the prompt back as the result, so the test can assert routing.
            vec![Message::user(prompt)]
        }
        async fn prepare_workspace(&self, _iso: &str, _id: &str) -> Result<Option<String>> {
            Ok(None)
        }
        async fn cleanup_workspace(&self, _ws: Option<&str>) -> bool {
            true
        }
        async fn record_events(&self, _task_id: &str, _events: &[agent_core::Event]) {}
        async fn on_complete(&self, _t: String, _c: String, _s: SubagentState, _o: String) {}
    }

    /// A host whose subagent output echoes the prompt (via a recording provider),
    /// so workflow routing is observable.
    struct PromptEchoHost;
    #[async_trait::async_trait]
    impl SubagentHost for PromptEchoHost {
        fn resolve_provider(&self, _tier: &str) -> Arc<dyn ModelProvider> {
            Arc::new(EchoProvider)
        }
        async fn capture_context(&self) -> String {
            String::new()
        }
        async fn child_registry(&self, _ws: Option<&str>) -> ToolRegistry {
            ToolRegistry::new()
        }
        async fn initial_messages(&self, _m: SubagentMode, _c: &str, prompt: &str) -> Vec<Message> {
            vec![Message::user(prompt)]
        }
        async fn prepare_workspace(&self, _i: &str, _id: &str) -> Result<Option<String>> {
            Ok(None)
        }
        async fn cleanup_workspace(&self, _ws: Option<&str>) -> bool {
            true
        }
        async fn record_events(&self, _task_id: &str, _e: &[agent_core::Event]) {}
        async fn on_complete(&self, _t: String, _c: String, _s: SubagentState, _o: String) {}
    }

    /// Provider that echoes the last user message as the assistant reply.
    struct EchoProvider;
    #[async_trait::async_trait]
    impl ModelProvider for EchoProvider {
        async fn complete(
            &self,
            messages: &[Message],
            _tools: &[ToolSpec],
        ) -> Result<ModelResponse> {
            let last = messages
                .iter()
                .rev()
                .find_map(|m| m.content.clone())
                .unwrap_or_default();
            Ok(ModelResponse::new(Message::assistant(last)))
        }
    }

    /// Provider that always fails, so its subagent run ends in `Failed`.
    struct FailProvider;
    #[async_trait::async_trait]
    impl ModelProvider for FailProvider {
        async fn complete(&self, _m: &[Message], _t: &[ToolSpec]) -> Result<ModelResponse> {
            Err(agent_core::CoreError::Provider("model exploded".into()))
        }
    }

    struct FailHost;
    #[async_trait::async_trait]
    impl SubagentHost for FailHost {
        fn resolve_provider(&self, _tier: &str) -> Arc<dyn ModelProvider> {
            Arc::new(FailProvider)
        }
        async fn capture_context(&self) -> String {
            String::new()
        }
        async fn child_registry(&self, _ws: Option<&str>) -> ToolRegistry {
            ToolRegistry::new()
        }
        async fn initial_messages(&self, _m: SubagentMode, _c: &str, prompt: &str) -> Vec<Message> {
            vec![Message::user(prompt)]
        }
        async fn prepare_workspace(&self, _i: &str, _id: &str) -> Result<Option<String>> {
            Ok(None)
        }
        async fn cleanup_workspace(&self, _ws: Option<&str>) -> bool {
            true
        }
        async fn record_events(&self, _t: &str, _e: &[agent_core::Event]) {}
        async fn on_complete(&self, _t: String, _c: String, _s: SubagentState, _o: String) {}
    }

    fn manager(host: Arc<dyn SubagentHost>) -> Arc<SubagentManager> {
        SubagentManager::new(host, Policy::FullAccess, 8)
    }

    #[tokio::test]
    async fn failed_agent_rejects_the_promise() {
        let mgr = manager(Arc::new(FailHost));
        // A failing subagent must reject `agent()` so the script can catch it,
        // rather than resolving with a bogus output.
        let script = r#"
            try { await agent({ prompt: "x" }); return "no-throw"; }
            catch (e) { return "caught"; }
        "#;
        let out = run_script(mgr, script).await.unwrap();
        assert_eq!(out["result"], "caught");
    }

    #[tokio::test]
    async fn runs_agent_and_parallel() {
        let mgr = manager(Arc::new(PromptEchoHost));
        let script = r#"
            const a = await agent({ prompt: "alpha" });
            const ps = await parallel([
                () => agent({ prompt: "p1" }),
                () => agent({ prompt: "p2" }),
            ]);
            log("done");
            return a + "|" + ps.join(",");
        "#;
        let out = run_script(mgr, script).await.unwrap();
        assert_eq!(out["result"], "alpha|p1,p2");
        assert_eq!(out["logs"], json!(["done"]));
    }

    #[tokio::test]
    async fn uncaught_error_is_reported_not_panic() {
        let mgr = manager(Arc::new(EchoHost));
        let out = run_script(mgr, "throw new Error('boom'); return 1;").await;
        assert!(out.is_err(), "uncaught error surfaces as Err");
        let msg = format!("{}", out.err().unwrap());
        assert!(msg.contains("boom") || msg.to_lowercase().contains("error"));
    }

    #[tokio::test]
    async fn empty_script_errors() {
        let mgr = manager(Arc::new(EchoHost));
        assert!(run_script(mgr, "   ").await.is_err());
    }
}
