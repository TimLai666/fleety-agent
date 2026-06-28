//! Windows Service Control Manager entry point for fleety-server.
//!
//! When the SCM starts `fleety-server run-service`, the process must speak the
//! service control protocol on its main thread within a few seconds. [`dispatch`]
//! hands control to the SCM, which calls [`service_main`] on its own thread;
//! there we register a control handler (Stop → graceful shutdown), claim the
//! single-instance pidfile, then run the server on a dedicated tokio runtime
//! until stopped.
//!
//! `windows-service`'s `define_windows_service!` macro expands without requiring
//! `unsafe` in this crate, so `#![forbid(unsafe_code)]` holds.

use std::ffi::OsString;
use std::time::Duration;

use windows_service::service::{
    ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::{define_windows_service, service_dispatcher};

const SERVICE_NAME: &str = "fleety-server";

define_windows_service!(ffi_service_main, service_main);

/// Hand control to the SCM. Errors if not started by the SCM (e.g. run by hand).
pub fn dispatch() -> windows_service::Result<()> {
    service_dispatcher::start(SERVICE_NAME, ffi_service_main)
}

fn service_main(_args: Vec<OsString>) {
    if let Err(e) = run_service() {
        tracing::error!(%e, "fleety-server windows service exited with error");
    }
}

fn run_service() -> windows_service::Result<()> {
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let event_handler = move |control| match control {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            let _ = stop_tx.send(true);
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    };
    let status_handle = service_control_handler::register(SERVICE_NAME, event_handler)?;

    let running = ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Running,
        controls_accepted: ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    };
    status_handle.set_service_status(running.clone())?;

    let _pid_guard = match fleety_tools::service::acquire(SERVICE_NAME) {
        Ok(fleety_tools::service::Acquire::Started(g)) => Some(g),
        Ok(fleety_tools::service::Acquire::AlreadyRunning(pid)) => {
            tracing::error!(
                pid,
                "another fleety-server is already running; service exiting"
            );
            set_stopped(&status_handle, &running, 0);
            return Ok(());
        }
        Err(e) => {
            tracing::warn!(report = ?e.report(), "pidfile check failed; continuing without it");
            None
        }
    };

    let exit_code = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => {
            rt.block_on(crate::run_server(Some(stop_rx)));
            0
        }
        Err(e) => {
            tracing::error!(%e, "cannot start tokio runtime in service");
            1
        }
    };

    set_stopped(&status_handle, &running, exit_code);
    Ok(())
}

fn set_stopped(
    handle: &service_control_handler::ServiceStatusHandle,
    base: &ServiceStatus,
    code: u32,
) {
    let stopped = ServiceStatus {
        current_state: ServiceState::Stopped,
        controls_accepted: ServiceControlAccept::empty(),
        exit_code: ServiceExitCode::Win32(code),
        ..base.clone()
    };
    if let Err(e) = handle.set_service_status(stopped) {
        tracing::warn!(%e, "could not report Stopped to SCM");
    }
}
