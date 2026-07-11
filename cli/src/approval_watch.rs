//! `cognos approval watch` — real-time HAL UI socket listener.

#[cfg(unix)]
mod imp {
    use std::collections::VecDeque;
    use std::io::{self, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::sync::{mpsc, Arc};
    use std::thread;
    use std::time::Duration;

    use cognos_ipc_grpc::approval_ui::{
        read_dialog, socket_parent_ready, write_dialog_response, UiDialogRequest,
        UiDialogResponse, DEFAULT_HAL_UI_SOCKET,
    };
    use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
    use tracing::{info, warn};

    use crate::commands::CliError;

    #[derive(Debug, Clone, clap::Args)]
    pub struct WatchArgs {
        #[arg(long, env = "COGNOS_HAL_UI_SOCKET")]
        pub socket: Option<PathBuf>,
    }

    struct PendingApproval {
        stream: UnixStream,
        dialog: UiDialogRequest,
    }

    enum UserChoice {
        Allow,
        Deny,
        AllowWithNotice(String),
        Quit,
    }

    pub async fn cmd_approval_watch(args: WatchArgs) -> Result<(), CliError> {
        let socket_path = args
            .socket
            .unwrap_or_else(|| PathBuf::from(DEFAULT_HAL_UI_SOCKET));

        tokio::task::spawn_blocking(move || run_watch_loop(&socket_path))
            .await
            .map_err(|e| CliError::ServiceError(format!("watch task failed: {e}")))??;
        Ok(())
    }

    fn run_watch_loop(socket_path: &Path) -> Result<(), CliError> {
        socket_parent_ready(socket_path).map_err(|e| {
            CliError::ServiceError(format!("cannot create socket parent: {e}"))
        })?;
        let _ = std::fs::remove_file(socket_path);
        let listener = UnixListener::bind(socket_path).map_err(|e| {
            CliError::ServiceError(format!(
                "cannot bind HAL UI socket {}: {e}",
                socket_path.display()
            ))
        })?;
        listener
            .set_nonblocking(true)
            .map_err(|e| CliError::ServiceError(e.to_string()))?;

        println!(
            "cognos approval watch — listening on {}\n\
             Per request: [a]llow  [d]eny  allow-with-[n]otice  [q]uit\n",
            socket_path.display()
        );
        info!(path = %socket_path.display(), "HAL UI watch started");

        let (connect_tx, connect_rx) = mpsc::channel::<UnixStream>();
        let listener = Arc::new(listener);
        let listener_clone = Arc::clone(&listener);
        let accept_handle = thread::spawn(move || {
            loop {
                match listener_clone.accept() {
                    Ok((stream, _)) => {
                        if connect_tx.send(stream).is_err() {
                            break;
                        }
                    }
                    Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(30));
                    }
                    Err(e) => {
                        warn!(error = %e, "accept error on HAL UI socket");
                        break;
                    }
                }
            }
        });

        let mut queue: VecDeque<PendingApproval> = VecDeque::new();
        enable_raw_mode().map_err(|e| CliError::ServiceError(e.to_string()))?;
        let result = watch_main_loop(&connect_rx, &mut queue);
        disable_raw_mode().ok();
        accept_handle.join().ok();
        result
    }

    fn watch_main_loop(
        connect_rx: &mpsc::Receiver<UnixStream>,
        queue: &mut VecDeque<PendingApproval>,
    ) -> Result<(), CliError> {
        let mut banner_shown = false;

        loop {
            while let Ok(mut stream) = connect_rx.try_recv() {
                match read_dialog(&mut stream) {
                    Ok(dialog) => {
                        if !queue.is_empty() {
                            println!(
                                "  + queued [{}] {} {} → {}",
                                dialog.short_id(),
                                dialog.level_label(),
                                dialog.agent,
                                dialog.target
                            );
                        }
                        queue.push_back(PendingApproval { stream, dialog });
                        banner_shown = false;
                    }
                    Err(e) => {
                        warn!(error = %e, "failed to read HAL UI dialog");
                        let _ = write_dialog_response(
                            &mut stream,
                            &UiDialogResponse {
                                approved: false,
                                notice: Some("malformed dialog".into()),
                            },
                        );
                    }
                }
            }

            if queue.is_empty() {
                banner_shown = false;
                if event::poll(Duration::from_millis(80)).map_err(|e| {
                    CliError::ServiceError(format!("keyboard poll: {e}"))
                })? {
                    if let Event::Key(key) = event::read().map_err(|e| {
                        CliError::ServiceError(format!("keyboard read: {e}"))
                    })? {
                        if key.code == KeyCode::Char('q') {
                            println!("\nwatch stopped (no pending requests).");
                            return Ok(());
                        }
                    }
                }
                continue;
            }

            if !banner_shown {
                if let Some(front) = queue.front() {
                    print_dialog(&front.dialog, queue.len().saturating_sub(1));
                    banner_shown = true;
                }
            }

            if let Some(choice) = poll_keyboard()? {
                let Some(mut pending) = queue.pop_front() else {
                    continue;
                };
                banner_shown = false;
                match choice {
                    UserChoice::Quit => {
                        let _ = write_dialog_response(
                            &mut pending.stream,
                            &UiDialogResponse {
                                approved: false,
                                notice: Some("watch quit".into()),
                            },
                        );
                        for mut rest in queue.drain(..) {
                            let _ = write_dialog_response(
                                &mut rest.stream,
                                &UiDialogResponse {
                                    approved: false,
                                    notice: Some("watch quit".into()),
                                },
                            );
                        }
                        println!("\nwatch stopped.");
                        return Ok(());
                    }
                    UserChoice::Allow => respond(
                        &mut pending,
                        UiDialogResponse {
                            approved: true,
                            notice: None,
                        },
                        "ALLOWED",
                    ),
                    UserChoice::Deny => respond(
                        &mut pending,
                        UiDialogResponse {
                            approved: false,
                            notice: None,
                        },
                        "DENIED",
                    ),
                    UserChoice::AllowWithNotice(notice) => {
                        let note = notice.clone();
                        respond(
                            &mut pending,
                            UiDialogResponse {
                                approved: true,
                                notice: Some(notice),
                            },
                            &format!("ALLOWED (notice: {note})"),
                        );
                    }
                }
            }
        }
    }

    fn respond(pending: &mut PendingApproval, resp: UiDialogResponse, label: &str) {
        let _ = write_dialog_response(&mut pending.stream, &resp);
        let d = &pending.dialog;
        println!(
            "→ {label} [{}] {} {} on {} (risk={:.2})",
            d.short_id(),
            d.level_label(),
            d.agent,
            d.target,
            d.hal_score
        );
        if let Some(note) = &resp.notice {
            println!("  notice: {note}");
        }
        io::stdout().flush().ok();
    }

    fn print_dialog(dialog: &UiDialogRequest, queued_behind: usize) {
        println!(
            "\n── HAL approval [{}] {} ──",
            dialog.short_id(),
            dialog.level_label()
        );
        println!("  action:  {}", dialog.action);
        println!("  target:  {}", dialog.target);
        println!("  agent:   {}", dialog.agent);
        println!("  risk:    {:.2}", dialog.hal_score);
        if dialog.is_ai_generated {
            println!("  AI-generated: yes");
        }
        if queued_behind > 0 {
            println!("  ({queued_behind} more in queue)");
        }
        print!("  [a]llow  [d]eny  [n]otice  [q]uit > ");
        io::stdout().flush().ok();
    }

    fn poll_keyboard() -> Result<Option<UserChoice>, CliError> {
        if !event::poll(Duration::from_millis(80)).map_err(|e| {
            CliError::ServiceError(format!("keyboard poll: {e}"))
        })? {
            return Ok(None);
        }
        let Event::Key(key) = event::read().map_err(|e| {
            CliError::ServiceError(format!("keyboard read: {e}"))
        })? else {
            return Ok(None);
        };
        Ok(map_key(key))
    }

    fn map_key(key: KeyEvent) -> Option<UserChoice> {
        let plain = !key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('a') | KeyCode::Char('A') if plain => Some(UserChoice::Allow),
            KeyCode::Char('d') | KeyCode::Char('D') if plain => Some(UserChoice::Deny),
            KeyCode::Char('n') | KeyCode::Char('N') if plain => {
                disable_raw_mode().ok();
                print!("\nNotice text: ");
                io::stdout().flush().ok();
                let mut line = String::new();
                let _ = io::stdin().read_line(&mut line);
                enable_raw_mode().ok();
                Some(UserChoice::AllowWithNotice(line.trim().to_string()))
            }
            KeyCode::Char('q') | KeyCode::Char('Q') if plain => Some(UserChoice::Quit),
            KeyCode::Esc if plain => Some(UserChoice::Deny),
            _ => None,
        }
    }
}

#[cfg(unix)]
pub use imp::{cmd_approval_watch, WatchArgs};

#[cfg(not(unix))]
#[derive(Debug, Clone, clap::Args)]
pub struct WatchArgs {
    #[arg(long)]
    pub socket: Option<std::path::PathBuf>,
}

#[cfg(not(unix))]
pub async fn cmd_approval_watch(_args: WatchArgs) -> Result<(), super::CliError> {
    Err(super::CliError::InvalidArgs(
        "cognos approval watch requires a Unix platform (WSL/Linux)".into(),
    ))
}
