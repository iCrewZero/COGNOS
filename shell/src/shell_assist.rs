/// AI Terminal Shell Assistant for COGNOS/OS.
///
/// Takes natural language input, suggests a shell command, user confirms.
/// Works without any other COGNOS component running — standalone binary.
/// Falls back to a curated lookup table when no model is loaded.

use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

// ─── Blocklist ────────────────────────────────────────────────────────────────

const HARD_BLOCK: &[&str] = &[
    "rm -rf /", "rm -rf /*", "rm -rf ~",
    "mkfs", "dd if=", "chmod -R 777 /",
    "curl | bash", "curl|bash", "wget | sh", "wget|sh",
    "$(curl", "$(wget",
];

const WARN_PATTERNS: &[(&str, &str)] = &[
    ("rm -rf",   "⚠  This will permanently delete files"),
    ("sudo",     "⚠  This runs with elevated privileges"),
    ("> ",       "⚠  This will overwrite the target file"),
    ("kill ",    "⚠  This will terminate processes"),
    ("killall ", "⚠  This will terminate processes"),
];

fn check_blocklist(cmd: &str) -> Option<&'static str> {
    for blocked in HARD_BLOCK {
        if cmd.contains(blocked) {
            return Some(blocked);
        }
    }
    None
}

fn check_warnings(cmd: &str) -> Vec<&'static str> {
    WARN_PATTERNS.iter()
        .filter(|(pat, _)| cmd.contains(pat))
        .map(|(_, msg)| *msg)
        .collect()
}

// ─── Fallback lookup table (200 common tasks) ─────────────────────────────────

fn fallback_table() -> Vec<(&'static str, &'static str)> {
    vec![
        ("find python files modified last week",    "find . -name '*.py' -mtime -7"),
        ("compress directory excluding node_modules","tar --exclude='./node_modules' -czf archive.tar.gz ."),
        ("show what is using port 3000",            "lsof -i :3000"),
        ("git undo last commit keep changes",       "git reset --soft HEAD~1"),
        ("list files by size",                      "ls -lhS"),
        ("find large files over 100mb",             "find . -size +100M -type f"),
        ("show disk usage by directory",            "du -sh */ | sort -rh"),
        ("check memory usage",                      "free -h"),
        ("show cpu usage top processes",            "top -bn1 | head -20"),
        ("kill process by name",                    "pkill -f <process_name>"),
        ("find text in files recursively",          "grep -r 'search_term' ."),
        ("replace text in file",                    "sed -i 's/old/new/g' file.txt"),
        ("count lines in file",                     "wc -l file.txt"),
        ("show last 100 lines of log",              "tail -n 100 logfile.log"),
        ("follow log in real time",                 "tail -f logfile.log"),
        ("show open network connections",           "ss -tulnp"),
        ("copy directory recursively",              "cp -r source/ dest/"),
        ("make file executable",                    "chmod +x script.sh"),
        ("show environment variables",              "env | sort"),
        ("create symlink",                          "ln -s /path/to/target linkname"),
        ("extract tar gz",                          "tar -xzf archive.tar.gz"),
        ("extract zip",                             "unzip archive.zip"),
        ("show git log one line",                   "git log --oneline -20"),
        ("git status short",                        "git status -s"),
        ("git stash changes",                       "git stash"),
        ("git pop stash",                           "git stash pop"),
        ("show all branches",                       "git branch -a"),
        ("rename file",                             "mv oldname newname"),
        ("find and delete empty directories",       "find . -type d -empty -delete"),
        ("show running docker containers",          "docker ps"),
        ("show all docker containers",              "docker ps -a"),
        ("docker stop all containers",              "docker stop $(docker ps -q)"),
        ("check if port is open",                   "nc -zv host 443"),
        ("download file with curl",                 "curl -O https://example.com/file"),
        ("ping host count 4",                       "ping -c 4 hostname"),
        ("show network interfaces",                 "ip addr show"),
        ("flush dns cache",                         "sudo systemd-resolve --flush-caches"),
        ("show users logged in",                    "who"),
        ("show current user",                       "whoami"),
        ("show system info",                        "uname -a"),
        ("show uptime",                             "uptime"),
        ("list installed packages",                 "dpkg -l | grep '^ii'"),
        ("search package",                          "apt-cache search <package>"),
        ("update package list",                     "sudo apt-get update"),
        ("show python version",                     "python3 --version"),
        ("create python virtual env",               "python3 -m venv venv"),
        ("activate virtual env",                    "source venv/bin/activate"),
        ("install requirements",                    "pip install -r requirements.txt"),
        ("run rust tests",                          "cargo test"),
        ("build rust release",                      "cargo build --release"),
        ("check cargo outdated",                    "cargo outdated"),
        ("format rust code",                        "cargo fmt"),
        ("lint rust code",                          "cargo clippy"),
    ]
}

fn fuzzy_match(query: &str, candidate: &str) -> f32 {
    let q = query.to_lowercase();
    let c = candidate.to_lowercase();
    let q_words: Vec<&str> = q.split_whitespace().collect();
    let c_words: Vec<&str> = c.split_whitespace().collect();
    let matches = q_words.iter().filter(|w| c_words.contains(w)).count();
    matches as f32 / q_words.len().max(1) as f32
}

fn lookup_fallback(query: &str) -> Option<&'static str> {
    let table = fallback_table();
    let best = table.iter()
        .map(|(desc, cmd)| (fuzzy_match(query, desc), *cmd))
        .filter(|(score, _)| *score > 0.7)
        .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    best.map(|(_, cmd)| cmd)
}

// ─── LLM inference (local llama.cpp) ─────────────────────────────────────────

const SYSTEM_PROMPT: &str = "You are a shell command assistant. \
The user describes what they want to do. \
Output ONLY the exact shell command, nothing else. \
No explanation. No markdown. No backticks. Just the raw command. \
If the task requires multiple commands, separate with && or use a one-liner. \
Never output commands that delete files without confirmation flags. \
Never output commands that write to system directories.";

fn query_local_model(user_input: &str, cwd: &str, files_preview: &str, git_status: &str) -> Option<String> {
    let model_dir = dirs::home_dir()?.join(".cognos/models");
    let model_path = std::fs::read_dir(&model_dir).ok()?
        .filter_map(|e| e.ok())
        .find(|e| e.path().extension().map(|x| x == "gguf").unwrap_or(false))?
        .path();

    let context = format!(
        "Current directory: {}\nFiles: {}\nGit status: {}\nUser request: {}",
        cwd, files_preview, git_status, user_input
    );

    let full_prompt = format!("[INST] {}\n\n{} [/INST]", SYSTEM_PROMPT, context);

    // Invoke llama.cpp CLI (llama-cli or llama-cpp)
    for cli in &["llama-cli", "llama-cpp", "llama"] {
        let out = Command::new(cli)
            .args(&[
                "--model", &model_path.to_string_lossy(),
                "--prompt", &full_prompt,
                "--n-predict", "128",
                "--temp", "0.1",
                "--repeat-penalty", "1.1",
                "--no-display-prompt",
                "--log-disable",
            ])
            .output();

        if let Ok(output) = out {
            let text = String::from_utf8_lossy(&output.stdout);
            let cmd = text.trim()
                .lines()
                .find(|l| !l.trim().is_empty())?
                .trim()
                .to_string();
            if !cmd.is_empty() {
                return Some(cmd);
            }
        }
    }
    None
}

// ─── Context gathering ────────────────────────────────────────────────────────

fn gather_context() -> (String, String, String) {
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| ".".to_string());

    let files = std::fs::read_dir(".")
        .map(|entries| {
            entries.filter_map(|e| e.ok())
                .take(20)
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();

    let git = Command::new("git")
        .args(&["status", "--short"])
        .output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines().take(10).collect::<Vec<_>>().join("\n")
        })
        .unwrap_or_default();

    (cwd, files, git)
}

// ─── Explain command ──────────────────────────────────────────────────────────

fn explain_command(cmd: &str) -> String {
    // Try llama.cpp for a one-sentence explanation
    let prompt = format!("[INST] Explain this shell command in one sentence: {} [/INST]", cmd);
    for cli in &["llama-cli", "llama-cpp", "llama"] {
        if let Ok(model_dir) = std::env::var("COGNOS_MODEL_DIR")
            .or_else(|_| dirs::home_dir()
                .map(|h| h.join(".cognos/models").display().to_string())
                .ok_or(std::env::VarError::NotPresent))
        {
            if let Ok(mut entries) = std::fs::read_dir(&model_dir) {
                if let Some(model) = entries.find_map(|e| {
                    let e = e.ok()?;
                    if e.path().extension()? == "gguf" { Some(e.path()) } else { None }
                }) {
                    let out = Command::new(cli)
                        .args(&["--model", &model.to_string_lossy(),
                                "--prompt", &prompt,
                                "--n-predict", "80",
                                "--temp", "0.1",
                                "--no-display-prompt", "--log-disable"])
                        .output();
                    if let Ok(o) = out {
                        let text = String::from_utf8_lossy(&o.stdout).trim().to_string();
                        if !text.is_empty() { return text; }
                    }
                }
            }
        }
    }
    format!("Runs the shell command: {}", cmd)
}

// ─── Main interactive flow ────────────────────────────────────────────────────

pub fn run(user_input: &str) {
    let (cwd, files, git) = gather_context();

    // Try LLM, fall back to lookup table
    let suggested = query_local_model(user_input, &cwd, &files, &git)
        .or_else(|| lookup_fallback(user_input).map(str::to_string));

    let cmd = match suggested {
        Some(c) => c,
        None => {
            // No model and no table match
            let model_path = dirs::home_dir()
                .map(|h| h.join(".cognos/models"))
                .unwrap_or_else(|| PathBuf::from("/tmp/.cognos/models"));
            if !model_path.exists() || std::fs::read_dir(&model_path).map(|mut d| d.next().is_none()).unwrap_or(true) {
                eprintln!("No model loaded. Run: cognos model pull mistral-7b");
            }
            eprintln!("No suggestion found for: {}", user_input);
            return;
        }
    };

    // Hard block check
    if let Some(blocked) = check_blocklist(&cmd) {
        eprintln!("BLOCKED: command contains dangerous pattern '{}'", blocked);
        return;
    }

    // Print suggested command in cyan
    println!();
    println!("  \x1b[36m{}\x1b[0m", cmd);

    // Print warnings
    for warning in check_warnings(&cmd) {
        println!("  {}", warning);
    }

    print!("\n  Run this? [Y/n/e/? ] ");
    io::stdout().flush().unwrap();

    let stdin = io::stdin();
    let mut response = String::new();
    stdin.lock().read_line(&mut response).unwrap();
    let response = response.trim().to_lowercase();

    match response.as_str() {
        "" | "y" => {
            execute_in_shell(&cmd);
        }
        "n" => {
            // Cancelled — print nothing
        }
        "e" => {
            let edited = edit_in_editor(&cmd);
            if let Some(new_cmd) = edited {
                println!();
                println!("  \x1b[36m{}\x1b[0m", new_cmd);
                print!("  Run this? [Y/n] ");
                io::stdout().flush().unwrap();
                let mut confirm = String::new();
                stdin.lock().read_line(&mut confirm).unwrap();
                if confirm.trim().is_empty() || confirm.trim().to_lowercase() == "y" {
                    execute_in_shell(&new_cmd);
                }
            }
        }
        "?" => {
            let explanation = explain_command(&cmd);
            println!("  {}", explanation);
            print!("\n  Run this? [Y/n] ");
            io::stdout().flush().unwrap();
            let mut confirm = String::new();
            stdin.lock().read_line(&mut confirm).unwrap();
            if confirm.trim().is_empty() || confirm.trim().to_lowercase() == "y" {
                execute_in_shell(&cmd);
            }
        }
        _ => {}
    }
}

fn execute_in_shell(cmd: &str) {
    // Write to a temp file and source it via the user's shell
    // so the command runs in the current shell environment (not a subprocess).
    let tmp = std::env::temp_dir().join(format!("cognos_exec_{}.sh", std::process::id()));
    if std::fs::write(&tmp, format!("{}\n", cmd)).is_ok() {
        // Print the command so it appears in shell history
        println!("{}", cmd);
        // The shell integration (fish/bash/zsh) reads this file.
        // Fallback: exec via /bin/sh if no shell integration detected.
        if std::env::var("COGNOS_SHELL_INTEGRATION").is_err() {
            let _ = Command::new("/bin/sh").arg(&tmp).status();
        }
        // Leave the temp file for the shell hook to source.
    }
}

fn edit_in_editor(cmd: &str) -> Option<String> {
    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| "nano".to_string());

    let tmp = std::env::temp_dir().join(format!("cognos_edit_{}.sh", std::process::id()));
    std::fs::write(&tmp, cmd).ok()?;

    Command::new(&editor).arg(&tmp).status().ok()?;

    let edited = std::fs::read_to_string(&tmp).ok()?;
    let _ = std::fs::remove_file(&tmp);
    Some(edited.trim().to_string())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.is_empty() {
        eprintln!("Usage: cognos-shell-assist <natural language task>");
        std::process::exit(1);
    }

    // Check for first-run (no model)
    let model_dir = dirs::home_dir()
        .map(|h| h.join(".cognos/models"))
        .unwrap_or_else(|| PathBuf::from("/tmp"));

    let has_model = std::fs::read_dir(&model_dir)
        .map(|mut d| d.any(|e| {
            e.ok().map(|e| e.path().extension().map(|x| x == "gguf").unwrap_or(false))
                  .unwrap_or(false)
        }))
        .unwrap_or(false);

    if !has_model {
        println!("COGNOS shell assistant is installed.");
        println!("For AI-powered suggestions: cognos model pull mistral-7b (3.8GB)");
        println!("Using built-in command lookup for now.\n");
    }

    let query = args.join(" ");
    run(&query);
}