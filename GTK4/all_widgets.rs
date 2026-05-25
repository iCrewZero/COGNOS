// shell/src/widgets/resource_monitor.rs
// Resource monitor widget for COGNOS/OS top bar (28px height).
// Reads /run/cognos/resources.json every 2 seconds via GLib timeout.
// Shows CPU%, RAM, battery with colour coding and AI usage indicator.

use gtk4::prelude::*;
use gtk4::{self as gtk, glib, Label, Box as GtkBox, Orientation, Tooltip};
use std::fs;
use std::time::Duration;

const RESOURCES_PATH: &str = "/run/cognos/resources.json";
const UPDATE_INTERVAL_SECS: u32 = 2;
const STALE_THRESHOLD_SECS: f64 = 10.0;

#[derive(Debug, Clone, Default, serde::Deserialize)]
struct ResourceData {
    cpu_percent: f64,
    ram_used_gb: f64,
    ram_total_gb: f64,
    battery_percent: Option<u32>,
    battery_charging: bool,
    ai_cpu_percent: f64,
    updated_at: Option<String>,
}

fn load_data() -> Option<ResourceData> {
    let raw = fs::read_to_string(RESOURCES_PATH).ok()?;
    serde_json::from_str(&raw).ok()
}

fn colour_for_cpu(pct: f64) -> &'static str {
    if pct > 80.0 { "#ef4444" }
    else if pct > 50.0 { "#f59e0b" }
    else { "rgba(148,163,184,0.7)" }
}

fn colour_for_ram(used: f64, total: f64) -> &'static str {
    let ratio = if total > 0.0 { used / total } else { 0.0 };
    if ratio > 0.9 { "#ef4444" }
    else if ratio > 0.7 { "#f59e0b" }
    else { "rgba(148,163,184,0.7)" }
}

fn colour_for_battery(pct: u32) -> &'static str {
    if pct < 10 { "#ef4444" }
    else if pct < 20 { "#f59e0b" }
    else { "rgba(148,163,184,0.7)" }
}

pub fn build_resource_monitor() -> GtkBox {
    let container = GtkBox::new(Orientation::Vertical, 0);
    container.set_margin_start(8);
    container.set_margin_end(8);

    // Main stats line: CPU X%  RAM X.XG  🔋XX%
    let main_label = Label::new(Some("CPU --%  RAM --G"));
    main_label.set_use_markup(true);
    main_label.add_css_class("resource-main");
    // monospace 11px
    main_label.set_markup("<span font_family='monospace' font='11'>CPU --%  RAM --G</span>");

    // AI sub-line (hidden when AI CPU <= 1%)
    let ai_label = Label::new(None);
    ai_label.set_use_markup(true);
    ai_label.set_visible(false);
    ai_label.add_css_class("resource-ai");

    container.append(&main_label);
    container.append(&ai_label);

    // Tooltip widget
    let tooltip_label = Label::new(Some("Loading..."));
    container.set_tooltip_widget(Some(&tooltip_label));
    container.set_has_tooltip(true);

    // Clone labels for closure
    let ml = main_label.clone();
    let al = ai_label.clone();
    let tl = tooltip_label.clone();

    // Poll every 2 seconds
    glib::timeout_add_seconds_local(UPDATE_INTERVAL_SECS, move || {
        let data = load_data().unwrap_or_default();

        // Build main text
        let cpu_color = colour_for_cpu(data.cpu_percent);
        let ram_color = colour_for_ram(data.ram_used_gb, data.ram_total_gb);

        let bat_str = match data.battery_percent {
            Some(pct) => {
                let icon = if data.battery_charging { "⚡" } else { "🔋" };
                let color = colour_for_battery(pct);
                format!("  <span color='{}'>{}{pct}%</span>", color, icon)
            }
            None => String::new(),
        };

        ml.set_markup(&format!(
            "<span font_family='monospace' font='11'>\
             <span color='{cpu_color}'>CPU {:.0}%</span>  \
             <span color='{ram_color}'>RAM {:.1}G</span>\
             {bat_str}</span>",
            data.cpu_percent, data.ram_used_gb,
            cpu_color = cpu_color, ram_color = ram_color,
        ));

        // AI sub-line: only show when > 1%
        if data.ai_cpu_percent > 1.0 {
            al.set_markup(&format!(
                "<span font_family='monospace' font='8' color='rgba(100,116,139,0.8)'>AI: {:.0}%</span>",
                data.ai_cpu_percent
            ));
            al.set_visible(true);
        } else {
            al.set_visible(false);
        }

        // Tooltip
        tl.set_text(&format!(
            "CPU: {:.0}%\nRAM: {:.1} / {:.1} GB\nAI budget: {:.0}% CPU\nBattery: {}",
            data.cpu_percent,
            data.ram_used_gb,
            data.ram_total_gb,
            data.ai_cpu_percent,
            data.battery_percent
                .map(|p| format!("{p}%"))
                .unwrap_or_else(|| "N/A".into()),
        ));

        glib::ControlFlow::Continue
    });

    container
}


// ── Agent Status Widget ───────────────────────────────────────────────────────
// shell/src/widgets/agent_status.rs
// Six coloured dots in the top bar, one per agent.
// Reads /run/cognos/ui-state.json every 500ms.

const UI_STATE_PATH: &str = "/run/cognos/ui-state.json";
const AGENT_ORDER: &[&str] = &["planner", "memory", "security", "scheduler", "file", "coding"];

#[derive(Debug, Clone, Default, serde::Deserialize)]
struct UiState {
    agents: std::collections::HashMap<String, String>,
}

fn colour_for_status(status: &str) -> &'static str {
    match status {
        "running"     => "#22c55e",
        "thinking"    => "#f59e0b",
        "alert"       => "#ef4444",
        "unavailable" => "#64748b",
        _             => "#1e2530",   // idle — nearly invisible
    }
}

fn is_pulsing(status: &str) -> bool {
    matches!(status, "thinking" | "alert")
}

pub fn build_agent_status() -> GtkBox {
    let container = GtkBox::new(Orientation::Horizontal, 4);
    container.set_margin_start(4);
    container.set_margin_end(4);

    // Create 6 drawing areas (dots) with CSS
    let dots: Vec<gtk::DrawingArea> = AGENT_ORDER.iter().map(|name| {
        let da = gtk::DrawingArea::new();
        da.set_content_width(6);
        da.set_content_height(6);
        da.set_widget_name(name);
        da.set_tooltip_text(Some(&format!("{}: idle", name)));
        container.append(&da);
        da
    }).collect();

    // Clone for closure
    let dots_ref = dots.clone();
    glib::timeout_add_local(Duration::from_millis(500), move || {
        let state: UiState = fs::read_to_string(UI_STATE_PATH)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();

        for (i, agent) in AGENT_ORDER.iter().enumerate() {
            let status = state.agents.get(*agent).map(|s| s.as_str()).unwrap_or("idle");
            let color = colour_for_status(status);
            let dot = &dots_ref[i];
            dot.set_tooltip_text(Some(&format!("{}: {}", agent, status)));

            // Update CSS colour via inline style — GTK4 approach
            let css = format!(
                "* {{ background-color: {}; border-radius: 3px; }}",
                color
            );
            let provider = gtk::CssProvider::new();
            provider.load_from_data(&css);
            dot.style_context().add_provider(
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }

        glib::ControlFlow::Continue
    });

    container
}


// ── Memory Browser Widget ─────────────────────────────────────────────────────
// shell/src/widgets/memory_browser.rs
// Floating window: three-panel browser of what COGNOS/OS knows about files.

pub fn build_memory_browser() -> gtk::Window {
    let win = gtk::Window::new();
    win.set_title(Some("Memory — what COGNOS/OS knows about your files"));
    win.set_default_size(680, 500);
    win.set_resizable(true);

    let outer = GtkBox::new(Orientation::Horizontal, 0);

    // ── Left panel: domain list (180px) ───────────────────────────────────────
    let left = GtkBox::new(Orientation::Vertical, 0);
    left.set_width_request(180);
    left.set_margin_top(8);
    left.set_margin_bottom(8);
    left.set_margin_start(8);

    let domain_header = Label::new(Some("Domains"));
    domain_header.set_halign(gtk::Align::Start);
    domain_header.add_css_class("heading");
    left.append(&domain_header);

    let domain_list = gtk::ListBox::new();
    domain_list.set_selection_mode(gtk::SelectionMode::Single);
    for domain in &["All files", "coding", "writing", "research", "other"] {
        let row = gtk::ListBoxRow::new();
        let lbl = Label::new(Some(domain));
        lbl.set_halign(gtk::Align::Start);
        lbl.set_margin_start(8);
        row.set_child(Some(&lbl));
        domain_list.append(&row);
    }
    left.append(&domain_list);

    // Stats at bottom of left panel
    let stats_label = Label::new(Some("Total: loading…"));
    stats_label.set_halign(gtk::Align::Start);
    stats_label.set_margin_top(8);
    stats_label.set_margin_start(8);
    stats_label.add_css_class("dim-label");
    left.append(&stats_label);

    // ── Centre panel: file list ────────────────────────────────────────────────
    let centre = GtkBox::new(Orientation::Vertical, 4);
    centre.set_hexpand(true);
    centre.set_margin_start(8);
    centre.set_margin_end(8);

    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some("Search files…"));
    centre.append(&search);

    let scrolled = gtk::ScrolledWindow::new();
    scrolled.set_vexpand(true);
    let file_list = gtk::ListBox::new();
    file_list.set_selection_mode(gtk::SelectionMode::Single);
    scrolled.set_child(Some(&file_list));
    centre.append(&scrolled);

    // Loading placeholder
    let loading_label = Label::new(Some("Loading memory data…"));
    loading_label.set_halign(gtk::Align::Center);
    loading_label.set_valign(gtk::Align::Center);
    file_list.append(&{
        let row = gtk::ListBoxRow::new();
        row.set_child(Some(&loading_label));
        row
    });

    // ── Right panel: file detail ───────────────────────────────────────────────
    let right = GtkBox::new(Orientation::Vertical, 4);
    right.set_width_request(200);
    right.set_margin_end(8);
    right.set_margin_top(8);

    let detail_label = Label::new(Some("Select a file to see details"));
    detail_label.set_wrap(true);
    detail_label.set_halign(gtk::Align::Start);
    detail_label.set_valign(gtk::Align::Start);
    right.append(&detail_label);

    // Action buttons
    let forget_btn = gtk::Button::with_label("Forget this file");
    forget_btn.set_tooltip_text(Some(
        "Removes from COGNOS memory only.\nYour actual file is NOT affected."
    ));
    let protect_btn = gtk::Button::with_label("Protect from index");
    protect_btn.set_margin_top(4);

    let actions = GtkBox::new(Orientation::Vertical, 4);
    actions.append(&forget_btn);
    actions.append(&protect_btn);
    actions.set_valign(gtk::Align::End);
    actions.set_vexpand(true);
    right.append(&actions);

    // Global actions
    let wipe_btn = gtk::Button::with_label("Forget everything");
    wipe_btn.add_css_class("destructive-action");
    wipe_btn.set_tooltip_text(Some("Wipes all COGNOS memory. Requires confirmation."));
    right.append(&wipe_btn);

    // ── Assemble ───────────────────────────────────────────────────────────────
    // Separator between panels
    let sep1 = gtk::Separator::new(Orientation::Vertical);
    let sep2 = gtk::Separator::new(Orientation::Vertical);

    outer.append(&left);
    outer.append(&sep1);
    outer.append(&centre);
    outer.append(&sep2);
    outer.append(&right);

    win.set_child(Some(&outer));
    win
}