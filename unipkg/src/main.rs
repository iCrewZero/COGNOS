/// UNIPKG v1 — Unified package manager for COGNOS/OS.
/// Wraps APT and Flatpak. No AI, no trust scoring in v1 — that's v2.
/// All existing `apt` and `flatpak` commands pass through unchanged.

use std::process::{Command, Stdio};
use std::io::{BufRead, BufReader};

#[derive(Debug, Clone)]
pub enum PackageSource { Apt, Flatpak }

#[derive(Debug, Clone)]
pub struct PackageResult {
    pub name: String,
    pub display_name: String,
    pub version: String,
    pub description: String,
    pub source: PackageSource,
    pub installed: bool,
    pub size_kb: Option<u64>,
}

// ─── APT source ───────────────────────────────────────────────────────────────

pub struct AptSource;

impl AptSource {
    pub fn available() -> bool {
        Command::new("apt-get").arg("--version")
            .stdout(Stdio::null()).stderr(Stdio::null())
            .status().map(|s| s.success()).unwrap_or(false)
    }

    pub fn search(query: &str) -> Vec<PackageResult> {
        let out = Command::new("apt-cache")
            .args(&["search", "--names-only", query])
            .output().unwrap_or_default();

        String::from_utf8_lossy(&out.stdout).lines()
            .filter_map(|line| {
                let mut parts = line.splitn(2, " - ");
                let name = parts.next()?.trim().to_string();
                let desc = parts.next().unwrap_or("").trim().to_string();
                Some(PackageResult {
                    name: name.clone(),
                    display_name: name.clone(),
                    version: String::new(),
                    description: desc,
                    source: PackageSource::Apt,
                    installed: Self::is_installed(&name),
                    size_kb: None,
                })
            })
            .collect()
    }

    pub fn install(name: &str) -> std::io::Result<bool> {
        let status = Command::new("apt-get")
            .args(&["install", "-y", name])
            .status()?;
        Ok(status.success())
    }

    pub fn remove(name: &str) -> std::io::Result<bool> {
        let status = Command::new("apt-get")
            .args(&["remove", "-y", name])
            .status()?;
        Ok(status.success())
    }

    pub fn update_all() -> std::io::Result<(bool, u32)> {
        Command::new("apt-get").args(&["update"]).status()?;
        let out = Command::new("apt-get")
            .args(&["upgrade", "-y"])
            .output()?;
        let count = String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| l.contains("upgraded"))
            .count() as u32;
        Ok((out.status.success(), count))
    }

    pub fn is_installed(name: &str) -> bool {
        Command::new("dpkg").args(&["-l", name])
            .stdout(Stdio::null()).stderr(Stdio::null())
            .status().map(|s| s.success()).unwrap_or(false)
    }
}

// ─── Flatpak source ───────────────────────────────────────────────────────────

pub struct FlatpakSource;

impl FlatpakSource {
    pub fn available() -> bool {
        Command::new("flatpak").arg("--version")
            .stdout(Stdio::null()).stderr(Stdio::null())
            .status().map(|s| s.success()).unwrap_or(false)
    }

    pub fn search(query: &str) -> Vec<PackageResult> {
        let out = Command::new("flatpak")
            .args(&["search", query, "--columns=name,application,version,description"])
            .output().unwrap_or_default();

        String::from_utf8_lossy(&out.stdout).lines()
            .skip(1) // header
            .filter_map(|line| {
                let cols: Vec<&str> = line.splitn(4, '\t').collect();
                if cols.len() < 2 { return None; }
                let name = cols[0].trim().to_string();
                let app_id = cols[1].trim().to_string();
                let version = cols.get(2).map(|s| s.trim().to_string()).unwrap_or_default();
                let desc = cols.get(3).map(|s| s.trim().to_string()).unwrap_or_default();
                Some(PackageResult {
                    name: app_id.clone(),
                    display_name: name,
                    version,
                    description: desc,
                    source: PackageSource::Flatpak,
                    installed: Self::is_installed(&app_id),
                    size_kb: None,
                })
            })
            .collect()
    }

    pub fn install(app_id: &str) -> std::io::Result<bool> {
        let status = Command::new("flatpak")
            .args(&["install", "-y", "flathub", app_id])
            .status()?;
        Ok(status.success())
    }

    pub fn remove(app_id: &str) -> std::io::Result<bool> {
        let status = Command::new("flatpak")
            .args(&["remove", "-y", app_id])
            .status()?;
        Ok(status.success())
    }

    pub fn update_all() -> std::io::Result<(bool, u32)> {
        let out = Command::new("flatpak").args(&["update", "-y"]).output()?;
        let count = String::from_utf8_lossy(&out.stdout)
            .lines().filter(|l| l.contains("Updated")).count() as u32;
        Ok((out.status.success(), count))
    }

    pub fn is_installed(app_id: &str) -> bool {
        let out = Command::new("flatpak")
            .args(&["list", "--app"])
            .output().unwrap_or_default();
        String::from_utf8_lossy(&out.stdout).contains(app_id)
    }
}

// ─── Resolver ─────────────────────────────────────────────────────────────────

pub struct Resolver;

impl Resolver {
    /// Search both sources, merge, deduplicate, score, return recommended first.
    pub fn resolve(query: &str) -> Vec<PackageResult> {
        let mut results: Vec<PackageResult> = Vec::new();

        if AptSource::available() {
            results.extend(AptSource::search(query));
        } else {
            eprintln!("Warning: apt-get not available — skipping APT source");
        }

        if FlatpakSource::available() {
            results.extend(FlatpakSource::search(query));
        } else {
            eprintln!("Warning: flatpak not available — skipping Flatpak source");
        }

        // Score each result
        let mut scored: Vec<(i32, PackageResult)> = results.into_iter()
            .map(|r| (Self::score(&r, query), r))
            .collect();

        scored.sort_by(|a, b| b.0.cmp(&a.0));
        scored.into_iter().map(|(_, r)| r).collect()
    }

    fn score(r: &PackageResult, query: &str) -> i32 {
        let mut s = 0i32;
        let name = r.name.to_lowercase();
        let q = query.to_lowercase();

        if name == q                    { s += 10; }
        else if name.starts_with(&q)    { s +=  5; }

        match r.source {
            PackageSource::Flatpak => s += 3, // preferred for desktop apps
            PackageSource::Apt     => s += 2, // preferred for CLI tools
        }

        if r.installed { s += 1; }

        let desc = r.description.to_lowercase();
        if desc.contains("deprecated") || desc.contains("obsolete") { s -= 5; }

        s
    }
}

// ─── CLI ──────────────────────────────────────────────────────────────────────

fn print_results(results: &[PackageResult]) {
    println!("{:<30} {:<10} {:<10} {:<10} {}",
             "NAME", "SOURCE", "VERSION", "INSTALLED", "DESCRIPTION");
    println!("{}", "-".repeat(85));
    for r in results {
        let src = match r.source { PackageSource::Apt => "APT", PackageSource::Flatpak => "Flatpak" };
        let inst = if r.installed { "✓" } else { "" };
        let desc: String = r.description.chars().take(40).collect();
        println!("{:<30} {:<10} {:<10} {:<10} {}",
                 r.display_name.chars().take(29).collect::<String>(),
                 src, r.version.chars().take(9).collect::<String>(),
                 inst, desc);
    }
}

pub fn run_cli(args: &[String]) {
    if args.is_empty() { return; }

    match args[0].as_str() {
        "search" if args.len() > 1 => {
            let query = &args[1..].join(" ");
            let results = Resolver::resolve(query);
            if results.is_empty() {
                println!("No packages found for '{}'", query);
            } else {
                print_results(&results);
            }
        }

        "install" if args.len() > 1 => {
            let query = &args[1..].join(" ");
            let results = Resolver::resolve(query);

            if results.is_empty() {
                eprintln!("No packages found for '{}'", query);
                std::process::exit(1);
            }

            let rec = &results[0];
            let src = match rec.source { PackageSource::Apt => "APT", PackageSource::Flatpak => "Flatpak" };
            println!("\n  Recommended: {} [{}]", rec.display_name, src);
            println!("  Version: {}  |  {}", rec.version, rec.description.chars().take(60).collect::<String>());
            if results.len() > 1 {
                let alt = &results[1];
                let asrc = match alt.source { PackageSource::Apt => "APT", PackageSource::Flatpak => "Flatpak" };
                println!("\n  Also available: {} [{}]  Version: {}", alt.display_name, asrc, alt.version);
            }

            print!("\n  Install recommended? [Y/n] ");
            use std::io::Write;
            std::io::stdout().flush().unwrap();
            let mut input = String::new();
            std::io::stdin().read_line(&mut input).unwrap();
            let input = input.trim().to_lowercase();

            if input.is_empty() || input == "y" {
                let ok = match rec.source {
                    PackageSource::Apt     => AptSource::install(&rec.name),
                    PackageSource::Flatpak => FlatpakSource::install(&rec.name),
                };
                match ok {
                    Ok(true)  => println!("Installed {}.", rec.display_name),
                    Ok(false) => { eprintln!("Installation failed."); std::process::exit(1); }
                    Err(e)    => { eprintln!("Error: {}", e); std::process::exit(1); }
                }
            } else {
                println!("Cancelled.");
            }
        }

        "remove" if args.len() > 1 => {
            let name = &args[1];
            let ok = AptSource::remove(name)
                .or_else(|_| FlatpakSource::remove(name));
            match ok {
                Ok(true)  => println!("Removed {}.", name),
                Ok(false) => eprintln!("Failed to remove {}.", name),
                Err(e)    => eprintln!("Error: {}", e),
            }
        }

        "update" => {
            if AptSource::available() {
                print!("[APT] Updating... ");
                std::io::stdout().flush().unwrap();
                match AptSource::update_all() {
                    Ok((_, n)) => println!("{} packages upgraded", n),
                    Err(e) => eprintln!("APT error: {}", e),
                }
            }
            if FlatpakSource::available() {
                print!("[Flatpak] Checking... ");
                std::io::stdout().flush().unwrap();
                match FlatpakSource::update_all() {
                    Ok((_, n)) => println!("{} apps updated", n),
                    Err(e) => eprintln!("Flatpak error: {}", e),
                }
            }
        }

        "list" => {
            let mut all: Vec<PackageResult> = Vec::new();
            if AptSource::available() {
                all.extend(AptSource::search("").into_iter().filter(|r| r.installed));
            }
            if FlatpakSource::available() {
                all.extend(FlatpakSource::search("").into_iter().filter(|r| r.installed));
            }
            if all.is_empty() {
                println!("No packages listed.");
            } else {
                print_results(&all);
            }
        }

        _ => {
            eprintln!("Usage: cognos install|remove|update|search|list [args]");
            std::process::exit(1);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // Passthrough: if args look like raw apt/flatpak, exec directly
    if args.first().map(|a| a == "apt" || a == "apt-get").unwrap_or(false) {
        let _ = Command::new("apt-get").args(&args[1..]).status();
        return;
    }
    if args.first().map(|a| a == "flatpak").unwrap_or(false) {
        let _ = Command::new("flatpak").args(&args[1..]).status();
        return;
    }

    run_cli(&args);
}
