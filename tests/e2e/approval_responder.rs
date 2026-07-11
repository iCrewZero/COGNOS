//! Test HAL UI socket auto-responder for E2E approval flows.

use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use cognos_ipc_grpc::approval_ui::{
    read_dialog, socket_parent_ready, write_dialog_response, UiDialogResponse,
};

/// How the auto-responder answers HAL UI dialogs.
#[derive(Debug, Clone, Copy)]
pub enum UiResponderMode {
    Approve,
    Deny,
    /// Hold the connection open until the orchestrator times out.
    Hang,
}

/// Background thread listening on the HAL UI socket path.
pub struct ApprovalUiResponder {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    pub socket_path: PathBuf,
}

impl ApprovalUiResponder {
    pub fn start(socket_path: PathBuf, mode: UiResponderMode) -> Self {
        socket_parent_ready(&socket_path).expect("ui socket parent");
        let _ = std::fs::remove_file(&socket_path);
        let listener = UnixListener::bind(&socket_path).expect("bind hal-ui socket");
        listener
            .set_nonblocking(true)
            .expect("nonblocking ui listener");
        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = Arc::clone(&stop);
        let handle = thread::spawn(move || responder_loop(listener, mode, stop_flag));
        Self {
            stop,
            handle: Some(handle),
            socket_path,
        }
    }
}

impl Drop for ApprovalUiResponder {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

fn responder_loop(listener: UnixListener, mode: UiResponderMode, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((mut stream, _)) => {
                if let Ok(dialog) = read_dialog(&mut stream) {
                    match mode {
                        UiResponderMode::Approve => {
                            let _ = write_dialog_response(
                                &mut stream,
                                &UiDialogResponse {
                                    approved: true,
                                    notice: Some(format!(
                                        "e2e auto-approved [{}]",
                                        dialog.short_id()
                                    )),
                                },
                            );
                        }
                        UiResponderMode::Deny => {
                            let _ = write_dialog_response(
                                &mut stream,
                                &UiDialogResponse {
                                    approved: false,
                                    notice: Some(format!(
                                        "e2e auto-denied [{}]",
                                        dialog.short_id()
                                    )),
                                },
                            );
                        }
                        UiResponderMode::Hang => {
                            // Hold the HAL connection open without responding.
                            while !stop.load(Ordering::Relaxed) {
                                thread::sleep(Duration::from_millis(100));
                            }
                        }
                    }
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(20));
            }
            Err(_) => break,
        }
    }
}

pub fn socket_dir(prefix: &str) -> PathBuf {
    let pid = std::process::id();
    PathBuf::from(format!("/tmp/cognos-e2e-{prefix}-{pid}"))
}

pub fn hal_gate_socket(dir: &Path) -> PathBuf {
    dir.join("hal.sock")
}

pub fn hal_ui_socket(dir: &Path) -> PathBuf {
    dir.join("hal-ui.sock")
}
