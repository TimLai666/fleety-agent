//! Never-crash boundaries.
//!
//! A single unit of work panicking must never take down the process. These
//! helpers run work and convert a panic into a structured [`CoreError`] instead
//! of unwinding into the caller.

use std::any::Any;
use std::panic::AssertUnwindSafe;

use crate::error::CoreError;

/// Run a synchronous closure, catching any panic and returning it as an error.
pub fn isolate<T>(f: impl FnOnce() -> T) -> Result<T, CoreError> {
    std::panic::catch_unwind(AssertUnwindSafe(f))
        .map_err(|payload| CoreError::Panic(payload_message(payload)))
}

/// Run a future on an isolated task, catching a panic (or task failure) as an error.
pub async fn isolate_async<F, T>(fut: F) -> Result<T, CoreError>
where
    F: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn(fut).await.map_err(join_error_to_core)
}

fn join_error_to_core(join_err: tokio::task::JoinError) -> CoreError {
    if join_err.is_panic() {
        CoreError::Panic(join_err.to_string())
    } else {
        CoreError::Task(join_err.to_string())
    }
}

fn payload_message(payload: Box<dyn Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isolate_returns_value() {
        assert!(matches!(isolate(|| 7), Ok(7)));
    }

    #[test]
    fn isolate_catches_panic() {
        let result = isolate(|| -> i32 { panic!("boom") });
        assert!(matches!(result, Err(CoreError::Panic(_))));
    }

    #[test]
    fn isolate_reports_owned_and_unknown_panic_payloads() {
        let owned = isolate(|| -> i32 { panic!("{}", String::from("owned boom")) });
        match owned {
            Err(CoreError::Panic(message)) => assert!(message.contains("owned boom")),
            other => panic!("unexpected result: {other:?}"),
        }

        let unknown = isolate(|| -> i32 { std::panic::panic_any(123_u8) });
        match unknown {
            Err(CoreError::Panic(message)) => assert_eq!(message, "unknown panic payload"),
            other => panic!("unexpected result: {other:?}"),
        }
    }

    #[tokio::test]
    async fn isolate_async_catches_panic() {
        let result: Result<(), CoreError> = isolate_async(async { panic!("async boom") }).await;
        assert!(matches!(result, Err(CoreError::Panic(_))));
    }

    #[tokio::test]
    async fn isolate_async_returns_value() {
        let result: Result<i32, CoreError> = isolate_async(async { 9 }).await;
        assert!(matches!(result, Ok(9)));
    }

    #[tokio::test]
    async fn join_error_to_core_reports_cancelled_tasks() {
        let handle = tokio::spawn(async {
            std::future::pending::<()>().await;
        });
        handle.abort();
        let join_err = handle.await.expect_err("aborted task");
        assert!(matches!(join_error_to_core(join_err), CoreError::Task(_)));
    }
}
