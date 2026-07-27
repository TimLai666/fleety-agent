//! Windows Service Control Manager entry point for fleetyd.
//!
//! When the SCM starts `fleetyd run-service`, the process must speak the service
//! control protocol on its main thread (report Running, accept Stop) within a
//! few seconds or the SCM kills it. [`dispatch`] hands control to the SCM, which
//! calls [`service_main`] on its own thread; there we register a control handler
//! (Stop → graceful shutdown), claim the single-instance pidfile, then run the
//! daemon on a dedicated tokio runtime until stopped.
//!
//! `windows-service`'s `define_windows_service!` macro expands without requiring
//! `unsafe` in this crate, so the workspace `#![forbid(unsafe_code)]` holds; the
//! FFI `unsafe` lives inside the dependency.

use std::ffi::OsString;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use windows_service::service::{
    ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::{define_windows_service, service_dispatcher};

const SERVICE_NAME: &str = "fleetyd";
const SERVICE_FAILURE: u32 = 1;
static SERVICE_EXIT_CODE: AtomicU32 = AtomicU32::new(SERVICE_FAILURE);

define_windows_service!(ffi_service_main, service_main);

/// Hand control to the SCM. Returns an error if the process was not started by
/// the SCM (e.g. run by hand) — the caller turns that into an actionable hint.
pub fn dispatch() -> windows_service::Result<u32> {
    SERVICE_EXIT_CODE.store(SERVICE_FAILURE, Ordering::Release);
    service_dispatcher::start(SERVICE_NAME, ffi_service_main)?;
    Ok(SERVICE_EXIT_CODE.load(Ordering::Acquire))
}

fn service_main(_args: Vec<OsString>) {
    let exit_code = match run_service() {
        Ok(exit_code) => exit_code,
        Err(e) => {
            tracing::error!(%e, "fleetyd windows service exited with error");
            SERVICE_FAILURE
        }
    };
    SERVICE_EXIT_CODE.store(exit_code, Ordering::Release);
}

fn run_service() -> windows_service::Result<u32> {
    // The control handler signals shutdown by flipping this watch to true; the
    // daemon's `run`/`serve` selects on it (see main::wait_stop).
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

    let starting = ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::StartPending,
        controls_accepted: ServiceControlAccept::empty(),
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 1,
        wait_hint: Duration::from_secs(10),
        process_id: None,
    };
    status_handle.set_service_status(starting.clone())?;

    // Reconnect control is the atomic outer owner. Claim it before the pidfile
    // so simultaneous service starts cannot race a dead-owner takeover.
    let control = match crate::ControlGuard::claim() {
        Ok(control) => control,
        Err(e) => {
            tracing::error!(report = ?e.report(), "cannot claim fleetyd reconnect control; exiting");
            set_stopped(&status_handle, &starting, SERVICE_FAILURE);
            return Ok(SERVICE_FAILURE);
        }
    };
    // Single-instance defense-in-depth under the already-held outer owner.
    let _pid_guard = match fleety_tools::service::acquire(SERVICE_NAME) {
        Ok(fleety_tools::service::Acquire::Started(g)) => Some(g),
        Ok(fleety_tools::service::Acquire::AlreadyRunning(pid)) => {
            tracing::error!(pid, "another fleetyd is already running; service exiting");
            set_stopped(&status_handle, &starting, SERVICE_FAILURE);
            return Ok(SERVICE_FAILURE);
        }
        Err(e) => {
            tracing::error!(report = ?e.report(), "cannot claim fleetyd service ownership; exiting");
            set_stopped(&status_handle, &starting, SERVICE_FAILURE);
            return Ok(SERVICE_FAILURE);
        }
    };

    let running = ServiceStatus {
        current_state: ServiceState::Running,
        controls_accepted: ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
        checkpoint: 0,
        wait_hint: Duration::default(),
        ..starting.clone()
    };
    status_handle.set_service_status(running.clone())?;

    tracing::info!(
        version = agent_core::VERSION,
        "fleetyd starting (windows service)"
    );
    let exit_code = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => {
            let res = rt.block_on(crate::run(Some(stop_rx), control));
            if let Err(e) = res {
                tracing::error!(report = ?e.report(), "fleetyd service run exited with error");
                1
            } else {
                0
            }
        }
        Err(e) => {
            tracing::error!(%e, "cannot start tokio runtime in service");
            1
        }
    };

    set_stopped(&status_handle, &running, exit_code);
    Ok(exit_code)
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
