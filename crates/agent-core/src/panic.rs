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
    match tokio::task::spawn(fut).await {
        Ok(value) => Ok(value),
        Err(join_err) if join_err.is_panic() => Err(CoreError::Panic(join_err.to_string())),
        Err(join_err) => Err(CoreError::Task(join_err.to_string())),
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
}
