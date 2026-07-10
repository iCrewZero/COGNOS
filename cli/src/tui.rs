//! TUI — interactive terminal UI built on ratatui. Shows agent statuses,
//! recent HAL decisions, memory activity, and system metrics in real time.
//! Keyboard-driven.
#![allow(dead_code)]
//!
//! The TUI subscribes to the COGNOS event bus (via an
//! [`EventBusReceiver`]) and renders four tabs:
//!
//! 1. **Agents**      — live agent roster with state, current task, heartbeat.
//! 2. **Decisions**   — recent HAL verdicts (Allow / Ask / Block / ...).
//! 3. **Memory**      — last indexed / forgotten memories.
//! 4. **Metrics**     — CPU / GPU / RAM / battery / scenario.
//!
//! Keys: `q` quit · `1`–`4` switch tabs · `↑`/`↓` navigate · `Enter` open
//! detail · `r` force refresh.
//!
//! v0: stub implementation.

use std::io::{self, Stdout};
use std::time::Duration;

use chrono::{DateTime, Utc};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Tabs};
use ratatui::Terminal;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

// ─── Errors ─────────────────────────────────────────────────────────────────

/// Errors that can arise while running the TUI.
#[derive(Debug, Error)]
pub enum TuiError {
    /// The terminal could not be initialised (raw mode / alternate screen).
    #[error("terminal init failed: {0}")]
    TerminalInit(String),
    /// An I/O error occurred while drawing or reading events.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// The event loop exited unexpectedly (e.g. event-bus receiver dropped).
    #[error("event loop: {0}")]
    EventLoop(String),
}

// ─── Tabs ───────────────────────────────────────────────────────────────────

/// Top-level tab in the TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tab {
    /// Agent roster.
    Agents,
    /// Recent HAL decisions.
    Decisions,
    /// Memory activity.
    Memory,
    /// System metrics.
    Metrics,
}

impl Tab {
    /// All tabs, in display order.
    pub const ALL: [Tab; 4] = [Tab::Agents, Tab::Decisions, Tab::Memory, Tab::Metrics];

    /// Human-readable title for the tab bar.
    pub fn title(self) -> &'static str {
        match self {
            Tab::Agents => "Agents",
            Tab::Decisions => "Decisions",
            Tab::Memory => "Memory",
            Tab::Metrics => "Metrics",
        }
    }

    /// Numeric index (1-based, matching the `1`–`4` keybindings).
    pub fn index(self) -> usize {
        match self {
            Tab::Agents => 0,
            Tab::Decisions => 1,
            Tab::Memory => 2,
            Tab::Metrics => 3,
        }
    }
}

impl Default for Tab {
    fn default() -> Self {
        Tab::Agents
    }
}

// ─── Domain placeholders ────────────────────────────────────────────────────

/// Per-agent state shown on the Agents tab.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentState {
    /// Agent id, e.g. "agent.coordinator".
    pub id: String,
    /// Lifecycle state: idle / running / blocked / crashed.
    pub state: String,
    /// Currently-executing task id, if any.
    pub current_task: Option<String>,
    /// Last heartbeat timestamp.
    pub last_heartbeat: Option<DateTime<Utc>>,
}

/// A HAL decision surfaced on the Decisions tab.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HalDecision {
    /// Monotonic decision id.
    pub id: u64,
    /// Agent that requested the gated action.
    pub agent: String,
    /// Capability being requested.
    pub capability: String,
    /// Verdict returned by the HAL (Allow / Ask / Block / ...).
    pub verdict: String,
    /// Risk score (0.0–1.0).
    pub risk: f32,
    /// When the verdict was issued.
    pub at: DateTime<Utc>,
}

/// System-wide metrics shown on the Metrics tab.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SystemMetrics {
    /// Per-core CPU usage (0.0–1.0).
    pub cpu_usage_per_core: Vec<f32>,
    /// GPU usage (0.0–1.0).
    pub gpu_usage: f32,
    /// RAM used, in GB.
    pub ram_used_gb: f32,
    /// Battery percentage (0–100). 0 when on AC.
    pub battery_percent: u8,
    /// True when discharging.
    pub battery_discharging: bool,
    /// Current detected scenario (e.g. "CodingActive").
    pub scenario: String,
}

// ─── Event bus receiver ─────────────────────────────────────────────────────

/// Events pushed into the TUI by the COGNOS event bus.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TuiEvent {
    /// Agent roster changed.
    AgentsUpdated(Vec<AgentState>),
    /// A new HAL decision was issued.
    HalDecision(HalDecision),
    /// Memory activity (indexed / forgotten).
    MemoryActivity { id: String, op: String },
    /// Metrics refreshed.
    Metrics(SystemMetrics),
    /// The event bus shut down — TUI should exit.
    Shutdown,
}

/// Receiver half of the TUI's event channel.
pub type EventBusReceiver = mpsc::Receiver<TuiEvent>;

// ─── TuiState ───────────────────────────────────────────────────────────────

/// All mutable state rendered by the TUI.
#[derive(Debug, Clone, Default)]
pub struct TuiState {
    /// Agent roster.
    pub agents: Vec<AgentState>,
    /// Recent HAL decisions (newest last).
    pub recent_decisions: Vec<HalDecision>,
    /// Latest metrics snapshot.
    pub metrics: SystemMetrics,
    /// Currently-selected tab.
    pub selected_tab: Tab,
    /// Index of the highlighted row inside the current tab.
    pub selected_row: usize,
}

impl TuiState {
    /// Move the cursor up by one (wraps to bottom).
    pub fn move_up(&mut self) {
        let len = self.current_len();
        if len == 0 {
            return;
        }
        self.selected_row = self.selected_row.checked_sub(1).unwrap_or(len - 1);
    }

    /// Move the cursor down by one (wraps to top).
    pub fn move_down(&mut self) {
        let len = self.current_len();
        if len == 0 {
            return;
        }
        self.selected_row = (self.selected_row + 1) % len;
    }

    /// Number of rows visible in the current tab.
    fn current_len(&self) -> usize {
        match self.selected_tab {
            Tab::Agents => self.agents.len(),
            Tab::Decisions => self.recent_decisions.len(),
            Tab::Memory => 0, // v0: no memory list wired yet — Owner: iCrewZero
            Tab::Metrics => 0,
        }
    }
}

// ─── Tui ────────────────────────────────────────────────────────────────────

/// The interactive terminal UI.
pub struct Tui {
    /// ratatui terminal handle (stdout-backed crossterm backend).
    pub terminal: Terminal<CrosstermBackend<Stdout>>,
    /// Rendered state.
    pub state: TuiState,
    /// Inbound event bus receiver.
    pub rx: EventBusReceiver,
    /// Set to true when the user requests to quit (`q` / Ctrl-C).
    pub quit: bool,
}

impl Tui {
    /// Construct a new TUI: enters raw mode, switches to the alternate
    /// screen, and opens an internal event channel (v0: unconnected).
    ///
    /// If Terminal::new() fails, the raw mode / alt screen are restored
    /// by the TerminalGuard so the user's shell isn't left in a broken state.
    /// Owner: iCrewZero
    pub fn new() -> Result<Self, TuiError> {
        // Guard: if we fail after enabling raw mode, restore the terminal.
        // Calling disable_raw_mode twice is harmless, so this is safe.
        struct TerminalGuard;
        impl Drop for TerminalGuard {
            fn drop(&mut self) {
                let _ = crossterm::terminal::disable_raw_mode();
                let _ = crossterm::execute!(io::stdout(), crossterm::terminal::LeaveAlternateScreen);
            }
        }
        let _guard = TerminalGuard;

        crossterm::terminal::enable_raw_mode()
            .map_err(|e| TuiError::TerminalInit(e.to_string()))?;
        crossterm::execute!(io::stdout(), crossterm::terminal::EnterAlternateScreen)
            .map_err(|e| TuiError::TerminalInit(e.to_string()))?;

        let backend = CrosstermBackend::new(io::stdout());
        let terminal = Terminal::new(backend).map_err(|e| TuiError::TerminalInit(e.to_string()))?;

        // v0: channel is created but never fed; v1 will subscribe to the
        // orchestrator event bus and forward events here.
        let (_tx, rx) = mpsc::channel::<TuiEvent>(64);

        Ok(Self {
            terminal,
            state: TuiState::default(),
            rx,
            quit: false,
        })
    }

    /// Main run loop. Polls crossterm events and event-bus messages with
    /// a short timeout, redraws on every tick, and exits when `quit` is
    /// set.
    pub async fn run(&mut self) -> Result<(), TuiError> {
        info!("TUI running");
        while !self.quit {
            self.draw()?;

            // TODO(v1): wire crossterm::event::poll + self.rx.recv() into
            //           a tokio::select! with a 250ms tick for live metrics.
            tokio::time::sleep(Duration::from_millis(250)).await;

            // v0: nothing feeds the channel, so we exit immediately after
            //     the first frame to avoid hanging the test harness. v1
            //     will block on the real event loop.
            warn!("TUI v0 stub — exiting after first frame");
            self.quit = true;
        }

        self.teardown()?;
        Ok(())
    }

    /// Dispatch a single event (keyboard or event-bus).
    pub fn handle_event(&mut self, event: TuiEvent) -> Result<(), TuiError> {
        match event {
            TuiEvent::AgentsUpdated(agents) => {
                self.state.agents = agents;
            }
            TuiEvent::HalDecision(d) => {
                self.state.recent_decisions.push(d);
                if self.state.recent_decisions.len() > 200 {
                    self.state.recent_decisions.remove(0);
                }
            }
            TuiEvent::MemoryActivity { id, op } => {
                debug!(%id, %op, "memory activity");
            }
            TuiEvent::Metrics(m) => {
                self.state.metrics = m;
            }
            TuiEvent::Shutdown => {
                self.quit = true;
            }
        }
        Ok(())
    }

    /// Handle a single crossterm keyboard event.
    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> Result<(), TuiError> {
        use crossterm::event::KeyCode;

        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => {
                self.quit = true;
            }
            KeyCode::Char('1') => self.state.selected_tab = Tab::Agents,
            KeyCode::Char('2') => self.state.selected_tab = Tab::Decisions,
            KeyCode::Char('3') => self.state.selected_tab = Tab::Memory,
            KeyCode::Char('4') => self.state.selected_tab = Tab::Metrics,
            KeyCode::Up | KeyCode::Char('k') => self.state.move_up(),
            KeyCode::Down | KeyCode::Char('j') => self.state.move_down(),
            KeyCode::Enter => {
                // TODO(v1): open a detail pane for the selected row.
                debug!(row = self.state.selected_row, "enter (v0 stub)");
            }
            KeyCode::Char('r') => {
                // TODO(v1): force-refresh via an RPC.
                debug!("refresh requested (v0 stub)");
            }
            _ => {}
        }
        Ok(())
    }

    /// Render a single frame.
    fn draw(&mut self) -> Result<(), TuiError> {
        self.terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(3), Constraint::Min(0), Constraint::Length(1)])
                .split(f.area());

            // Tab bar.
            let titles: Vec<Line> = Tab::ALL
                .iter()
                .map(|t| Line::from(t.title()))
                .collect();
            let tabs = Tabs::new(titles)
                .block(Block::default().borders(Borders::ALL).title("COGNOS"))
                .select(self.state.selected_tab.index())
                .style(Style::default().fg(Color::White))
                .highlight_style(Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan));
            f.render_widget(tabs, chunks[0]);

            // Body — chosen by selected tab (render per arm: List vs Paragraph).
            match self.state.selected_tab {
                Tab::Agents => {
                    let items: Vec<ListItem> = self
                        .state
                        .agents
                        .iter()
                        .map(|a| {
                            ListItem::new(format!(
                                "{:<20} {:<10} {}",
                                a.id,
                                a.state,
                                a.current_task.as_deref().unwrap_or("-"),
                            ))
                        })
                        .collect();
                    let list = List::new(items)
                        .block(Block::default().borders(Borders::ALL).title("Agents"));
                    f.render_widget(list, chunks[1]);
                }
                Tab::Decisions => {
                    let items: Vec<ListItem> = self
                        .state
                        .recent_decisions
                        .iter()
                        .map(|d| {
                            ListItem::new(format!(
                                "{:<6} {:<20} {:<20} {:<8} {:>5.2}",
                                d.id, d.agent, d.capability, d.verdict, d.risk,
                            ))
                        })
                        .collect();
                    let list = List::new(items)
                        .block(Block::default().borders(Borders::ALL).title("Decisions"));
                    f.render_widget(list, chunks[1]);
                }
                Tab::Memory => {
                    // TODO(v1): dedicated memory feed list.
                    let para = Paragraph::new("(v0 stub — memory activity list not yet wired)")
                        .block(Block::default().borders(Borders::ALL).title("Memory"));
                    f.render_widget(para, chunks[1]);
                }
                Tab::Metrics => {
                    let m = &self.state.metrics;
                    let para = Paragraph::new(format!(
                        "scenario:  {}\ncpu cores: {}\ngpu:       {:.2}\nram:       {:.2} GB\nbattery:   {}% {}\n",
                        m.scenario,
                        m.cpu_usage_per_core.len(),
                        m.gpu_usage,
                        m.ram_used_gb,
                        m.battery_percent,
                        if m.battery_discharging { "(discharging)" } else { "(on ac)" },
                    ))
                    .block(Block::default().borders(Borders::ALL).title("Metrics"));
                    f.render_widget(para, chunks[1]);
                }
            }

            // Footer / help line.
            let help = Paragraph::new("q quit · 1-4 tabs · ↑↓ navigate · Enter detail · r refresh");
            f.render_widget(help, chunks[2]);
        })?;
        Ok(())
    }

    /// Restore the terminal to its pre-TUI state.
    fn teardown(&mut self) -> Result<(), TuiError> {
        crossterm::execute!(io::stdout(), crossterm::terminal::LeaveAlternateScreen)
            .map_err(TuiError::Io)?;
        crossterm::terminal::disable_raw_mode().map_err(TuiError::Io)?;
        Ok(())
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        // Best-effort restore — ignore errors.
        let _ = crossterm::execute!(io::stdout(), crossterm::terminal::LeaveAlternateScreen);
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

// v0: stub implementation
