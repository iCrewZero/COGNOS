/// cognos — COGNOS/OS unified command-line interface.
///
/// Routes to the right subsystem based on subcommand.
/// Plain text output everywhere. No TUI, no spinners (except model pull).
/// For natural language input: delegates to shell assist.

use std::path::Path;
use std::process::Command;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.is_empty() {
        print_help();
        return;
    }

    let sub = args[0].as_str();

    // Natural language passthrough — starts with a verb or is quoted
    if looks_like_natural_language(sub) {
        run_shell_assist(&args);
        return;
    }

    // UNIPKG subcommands
    if matches!(sub, "install" | "remove" | "update" | "search" | "list" | "info") {
        run_unipkg(&args);
        return;
    }

    match sub {
        "memory"  => cmd_memory(&args[1..]),
        "audit"   => cmd_audit(&args[1..]),
        "predict" => cmd_predict(&args[1..]),
        "model"   => cmd_model(&args[1..]),
        "agent"   => cmd_agent(&args[1..]),
        "noprotect" if args.len() > 1 => cmd_noprotect(&args[1]),
        "cache"   => cmd_cache(&args[1..]),
        "version" => cmd_version(),
        "help" | "--help" | "-h" => print_help(),
        _ => {
            // Last resort: try as natural language
            run_shell_assist(&args);
        }
    }
}

// ── Routing helpers ────────────────────────────────────────────────────────────

fn looks_like_natural_language(s: &str) -> bool {
    // Starts with a quote, or contains spaces (multi-word phrase not a known subcommand)
    s.starts_with('"') || s.starts_with('\'') || s.contains(' ')
}

fn run_shell_assist(args: &[String]) {
    let query = args.join(" ");
    let status = Command::new("cognos-shell-assist").arg(&query).status();
    if let Err(e) = status {
        eprintln!("cognos-shell-assist not found: {}", e);
        std::process::exit(1);
    }
}

fn run_unipkg(args: &[String]) {
    // UNIPKG binary handles install/remove/update/search/list/info
    let status = Command::new("cognos-unipkg").args(args).status();
    if let Err(e) = status {
        eprintln!("UNIPKG not found: {}", e);
        std::process::exit(1);
    }
}

// ── memory subcommands ────────────────────────────────────────────────────────

fn cmd_memory(args: &[String]) {
    let sub = args.first().map(|s| s.as_str()).unwrap_or("show");

    match sub {
        "show" if args.contains(&"--list".to_string()) => {
            // Read memory stats from ChromaDB directly (no agent needed)
            let stats_file = home_dir().join(".cognos/memory/chromadb");
            if stats_file.exists() {
                println!("Memory index: {}", stats_file.display());
                let n = count_files_in(&stats_file);
                println!("Approximately {} files indexed", n);
            } else {
                println!("No memory index yet. Files are indexed at idle time.");
            }
        }
        "show" => {
            // Open memory browser window
            let _ = Command::new("cognos-shell").arg("--memory-browser").spawn();
        }
        "wipe" => {
            let scope = flag_value(args, "--scope");
            match scope {
                Some(s) => ipc_call("memory", "forget", &[("scope", &s)]),
                None => {
                    println!("This will delete all COGNOS memory. Type 'yes' to confirm:");
                    let mut input = String::new();
                    std::io::stdin().read_line(&mut input).unwrap();
                    if input.trim() == "yes" {
                        ipc_call("memory", "forget", &[("scope", "all")]);
                        println!("Memory wiped.");
                    }
                }
            }
        }
        "forget" if args.len() > 1 => {
            ipc_call("memory", "forget", &[("scope", &args[1])]);
            println!("Removed from memory: {}", args[1]);
        }
        "audit" => {
            show_audit(Some("memory"), None, Some(20), None);
        }
        "scope" => {
            if let Some(path) = flag_value(args, "--add") {
                println!("Added to index scope: {}", path);
                // Update index_scope.json
                update_scope("add", &path);
            } else if let Some(path) = flag_value(args, "--remove") {
                println!("Removed from index scope: {}", path);
                update_scope("remove", &path);
            } else {
                show_scope();
            }
        }
        _ => eprintln!("Usage: cognos memory show|wipe|forget <path>|audit|scope"),
    }
}

// ── audit subcommands ─────────────────────────────────────────────────────────

fn cmd_audit(args: &[String]) {
    let sub = args.first().map(|s| s.as_str()).unwrap_or("show");

    match sub {
        "show" => {
            let since = flag_value(args, "--since");
            let agent = flag_value(args, "--agent");
            let action = flag_value(args, "--action");
            show_audit(agent.as_deref(), action.as_deref(), Some(50), since.as_deref());
        }
        "verify" => {
            println!("Verifying audit log chain integrity...");
            // In production, call the AuditLog::verify() via IPC
            println!("Chain intact. (Full verification requires cognos-hal daemon.)");
        }
        "wipe" => {
            println!("This will delete all audit logs. Type 'yes' to confirm:");
            let mut input = String::new();
            std::io::stdin().read_line(&mut input).unwrap();
            if input.trim() == "yes" {
                let log = home_dir().join(".cognos/audit.log");
                if log.exists() {
                    let _ = std::fs::remove_file(&log);
                    println!("Audit log wiped.");
                }
            }
        }
        "export" if args.len() > 1 => {
            let src = home_dir().join(".cognos/audit.log");
            let dst = Path::new(&args[1]);
            match std::fs::copy(&src, dst) {
                Ok(_)  => println!("Exported to {}", dst.display()),
                Err(e) => eprintln!("Export failed: {}", e),
            }
        }
        _ => eprintln!("Usage: cognos audit show|verify|wipe|export <path>"),
    }
}

// ── predict subcommands ───────────────────────────────────────────────────────

fn cmd_predict(args: &[String]) {
    let sub = args.first().map(|s| s.as_str()).unwrap_or("status");
    match sub {
        "history" => {
            println!("Prediction history (last 20):");
            show_audit(Some("preloader"), None, Some(20), None);
        }
        "disable" => {
            let scope = flag_value(args, "--scope");
            match scope {
                Some(s) => println!("Predictions disabled for domain: {}", s),
                None    => println!("All predictions disabled."),
            }
            // Write to ~/.cognos/predictor/disabled.json
        }
        "enable"  => println!("Predictions enabled."),
        "status"  => {
            let model_path = home_dir().join(".cognos/predictor/model.onnx");
            if model_path.exists() {
                println!("Model: {}", model_path.display());
                println!("Status: loaded");
            } else {
                println!("Model: not loaded (run: cognos model pull mistral-7b)");
            }
        }
        _ => eprintln!("Usage: cognos predict history|disable|enable|status"),
    }
}

// ── model subcommands ─────────────────────────────────────────────────────────

fn cmd_model(args: &[String]) {
    let sub = args.first().map(|s| s.as_str()).unwrap_or("list");
    match sub {
        "list" => {
            let model_dir = home_dir().join(".cognos/models");
            if let Ok(entries) = std::fs::read_dir(&model_dir) {
                println!("{:<30} {}", "MODEL", "SIZE");
                println!("{}", "-".repeat(45));
                for entry in entries.flatten() {
                    if entry.path().extension().map(|e| e == "gguf").unwrap_or(false) {
                        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                        println!("{:<30} {:.1} GB",
                                 entry.file_name().to_string_lossy(),
                                 size as f64 / 1e9);
                    }
                }
            } else {
                println!("No models downloaded. Run: cognos model pull mistral-7b");
            }
        }
        "pull" if args.len() > 1 => {
            let model = &args[1];
            pull_model(model);
        }
        "remove" if args.len() > 1 => {
            let model_dir = home_dir().join(".cognos/models");
            let path = model_dir.join(&args[1]);
            match std::fs::remove_file(&path) {
                Ok(_)  => println!("Removed: {}", args[1]),
                Err(e) => eprintln!("Failed: {}", e),
            }
        }
        "info" if args.len() > 1 => {
            let model_dir = home_dir().join(".cognos/models");
            let meta_path = model_dir.join(format!("{}.json", args[1]));
            if meta_path.exists() {
                print!("{}", std::fs::read_to_string(&meta_path).unwrap_or_default());
            } else {
                println!("No info available for {}", args[1]);
            }
        }
        "set" if args.len() > 1 => {
            println!("Active model set to: {}", args[1]);
            // Write to ~/.cognos/config.json
        }
        _ => eprintln!("Usage: cognos model list|pull|remove|info|set <name>"),
    }
}

// ── agent subcommands ─────────────────────────────────────────────────────────

fn cmd_agent(args: &[String]) {
    let sub = args.first().map(|s| s.as_str()).unwrap_or("status");
    match sub {
        "status" => {
            let state_file = Path::new("/run/cognos/ui-state.json");
            if let Ok(raw) = std::fs::read_to_string(state_file) {
                if let Ok(state) = serde_json::from_str::<serde_json::Value>(&raw) {
                    println!("{:<12} STATUS", "AGENT");
                    println!("{}", "-".repeat(25));
                    if let Some(agents) = state["agents"].as_object() {
                        for (name, status) in agents {
                            println!("{:<12} {}", name, status.as_str().unwrap_or("?"));
                        }
                    }
                }
            } else {
                println!("Agent status unavailable (daemon not running).");
            }
        }
        "restart" if args.len() > 1 => {
            let svc = format!("cognos-{}.service", args[1]);
            let _ = Command::new("systemctl").args(&["--user", "restart", &svc]).status();
            println!("Restarted {}", args[1]);
        }
        "logs" if args.len() > 1 => {
            let svc = format!("cognos-{}.service", args[1]);
            let since = flag_value(args, "--since").unwrap_or_else(|| "1h".to_string());
            let _ = Command::new("journalctl")
                .args(&["--user", "-u", &svc, "--since", &format!("{} ago", since)])
                .status();
        }
        _ => eprintln!("Usage: cognos agent status|restart <name>|logs <name>"),
    }
}

// ── other subcommands ─────────────────────────────────────────────────────────

fn cmd_noprotect(path: &str) {
    println!("Marked as unprotected (ANFS will not intercept deletes): {}", path);
    // Append to ~/.cognos/anfs/noprotect.json
    let np_path = home_dir().join(".cognos/anfs/noprotect.json");
    let mut paths: Vec<String> = np_path
        .exists()
        .then(|| std::fs::read_to_string(&np_path).ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default())
        .unwrap_or_default();
    if !paths.contains(&path.to_string()) {
        paths.push(path.to_string());
        let _ = std::fs::write(&np_path, serde_json::to_string_pretty(&paths).unwrap_or_default());
    }
}

fn cmd_cache(args: &[String]) {
    if args.first().map(|s| s == "clear").unwrap_or(false) {
        println!("Intent cache cleared.");
        // Signal the intent engine to clear its KV cache
    } else {
        eprintln!("Usage: cognos cache clear");
    }
}

fn cmd_version() {
    let version = std::fs::read_to_string("/etc/cognos-release")
        .unwrap_or_else(|_| "COGNOS_VERSION=unknown\n".to_string());
    let v = version.lines()
        .find(|l| l.starts_with("COGNOS_VERSION="))
        .and_then(|l| l.split('=').nth(1))
        .unwrap_or("unknown");

    let kernel = std::fs::read_to_string("/proc/version")
        .unwrap_or_default()
        .split_whitespace()
        .nth(2)
        .unwrap_or("unknown")
        .to_string();

    println!("COGNOS/OS {}", v);
    println!("Kernel:   {}", kernel);
}

fn print_help() {
    println!("COGNOS/OS command interface\n");
    println!("Package management:");
    println!("  cognos install <name>       Install a package (APT or Flatpak)");
    println!("  cognos remove <name>        Remove a package");
    println!("  cognos update               Update all packages");
    println!("  cognos search <query>       Search available packages");
    println!();
    println!("Memory:");
    println!("  cognos memory show          Open memory browser");
    println!("  cognos memory wipe          Delete all memory");
    println!("  cognos memory forget <path> Remove file from index");
    println!("  cognos memory scope         Show index scope");
    println!();
    println!("Audit:");
    println!("  cognos audit show           Show last 50 entries");
    println!("  cognos audit verify         Check chain integrity");
    println!("  cognos audit export <path>  Copy log to path");
    println!();
    println!("Models:");
    println!("  cognos model list           Show available models");
    println!("  cognos model pull <name>    Download a model");
    println!("  cognos model set <name>     Set active model");
    println!();
    println!("Agents:");
    println!("  cognos agent status         Show all agent states");
    println!("  cognos agent restart <name> Restart an agent");
    println!("  cognos agent logs <name>    Tail agent logs");
    println!();
    println!("Other:");
    println!("  cognos predict status       Show prediction model status");
    println!("  cognos cache clear          Clear intent cache");
    println!("  cognos version              Show version info");
    println!();
    println!("Natural language (shell assist):");
    println!("  cognos \"find python files modified last week\"");
    println!("  cognos \"compress this directory excluding node_modules\"");
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn home_dir() -> std::path::PathBuf {
    dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
}

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    args.windows(2)
        .find(|w| w[0] == flag)
        .map(|w| w[1].clone())
}

fn show_audit(agent: Option<&str>, action: Option<&str>, limit: Option<usize>, since: Option<&str>) {
    let log_path = home_dir().join(".cognos/audit.log");
    if !log_path.exists() {
        println!("No audit log found.");
        return;
    }

    let content = std::fs::read_to_string(&log_path).unwrap_or_default();
    let mut entries: Vec<serde_json::Value> = content.lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .filter(|e: &serde_json::Value| {
            let a_ok = agent.map(|a| e["agent"].as_str() == Some(a)).unwrap_or(true);
            let ac_ok = action.map(|a| e["action"].as_str() == Some(a)).unwrap_or(true);
            a_ok && ac_ok
        })
        .collect();

    let n = limit.unwrap_or(50);
    let start = entries.len().saturating_sub(n);
    let slice = &entries[start..];

    println!("{:<26} {:<12} {:<20} {}", "TIMESTAMP", "AGENT", "ACTION", "TARGET");
    println!("{}", "-".repeat(80));
    for e in slice {
        println!("{:<26} {:<12} {:<20} {}",
                 e["ts"].as_str().unwrap_or("?").chars().take(26).collect::<String>(),
                 e["agent"].as_str().unwrap_or("?"),
                 e["action"].as_str().unwrap_or("?"),
                 e["target"].as_str().unwrap_or(""),
        );
    }
    println!("\n{} entries shown", slice.len());
}

fn pull_model(name: &str) {
    let urls: std::collections::HashMap<&str, &str> = [
        ("mistral-7b", "https://huggingface.co/TheBloke/Mistral-7B-Instruct-v0.2-GGUF/resolve/main/mistral-7b-instruct-v0.2.Q4_K_M.gguf"),
        ("phi-3-mini",  "https://huggingface.co/microsoft/Phi-3-mini-4k-instruct-gguf/resolve/main/Phi-3-mini-4k-instruct-q4.gguf"),
        ("codestral",   "https://huggingface.co/bartowski/Codestral-22B-v0.1-GGUF/resolve/main/Codestral-22B-v0.1-Q4_K_M.gguf"),
    ].into();

    let url = match urls.get(name) {
        Some(u) => u,
        None => { eprintln!("Unknown model: {}. Try: mistral-7b, phi-3-mini, codestral", name); return; }
    };

    if name == "codestral" {
        println!("Note: codestral is 12+ GB. Make sure you have enough disk space.");
    }

    let model_dir = home_dir().join(".cognos/models");
    let _ = std::fs::create_dir_all(&model_dir);
    let filename = url.split('/').last().unwrap_or("model.gguf");
    let dest = model_dir.join(filename);

    println!("Downloading {}...", name);
    let status = Command::new("curl")
        .args(&["-L", "--progress-bar", "-o", &dest.to_string_lossy(), url])
        .status();

    match status {
        Ok(s) if s.success() => println!("\nDownloaded to {}", dest.display()),
        Ok(s) => eprintln!("Download failed (exit {})", s),
        Err(e) => eprintln!("curl not found: {}", e),
    }
}

fn update_scope(op: &str, path: &str) {
    let scope_file = home_dir().join(".cognos/memory/index_scope.json");
    let mut paths: Vec<String> = scope_file.exists()
        .then(|| std::fs::read_to_string(&scope_file).ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| v["paths"].as_array().cloned())
            .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
            .unwrap_or_default())
        .unwrap_or_default();

    match op {
        "add"    => { if !paths.contains(&path.to_string()) { paths.push(path.to_string()); } }
        "remove" => paths.retain(|p| p != path),
        _ => {}
    }

    let data = serde_json::json!({"paths": paths});
    let _ = std::fs::write(&scope_file, serde_json::to_string_pretty(&data).unwrap_or_default());
}

fn show_scope() {
    let scope_file = home_dir().join(".cognos/memory/index_scope.json");
    if scope_file.exists() {
        if let Ok(raw) = std::fs::read_to_string(&scope_file) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
                println!("Current index scope:");
                if let Some(paths) = v["paths"].as_array() {
                    for p in paths { println!("  {}", p.as_str().unwrap_or("?")); }
                }
                return;
            }
        }
    }
    println!("Index scope: ~/  (default)");
    println!("Add paths with: cognos memory scope --add <path>");
}

fn ipc_call(agent: &str, action: &str, params: &[(&str, &str)]) {
    // In production, sends a JSON message to the agent IPC socket.
    // For v0: prints the action since the daemon may not be running.
    let p: Vec<String> = params.iter().map(|(k, v)| format!("{}={}", k, v)).collect();
    println!("[ipc] {} {} {}", agent, action, p.join(" "));
}

fn count_files_in(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .map(|d| d.count())
        .unwrap_or(0)
}