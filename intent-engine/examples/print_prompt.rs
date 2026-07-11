//! Emit `build_prompt(input, default_session)` on stdout for curl probes.
use cognos_intent_engine::prompt::build_prompt;
use cognos_intent_engine::schema_validator::SessionContext;

fn main() {
    let input = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: print_prompt <user input>");
        std::process::exit(2);
    });
    let ctx = SessionContext {
        last_active_domain: None,
        last_active_files: vec![],
        current_time: chrono::Local::now().format("%H:%M").to_string(),
        time_since_last_session: None,
    };
    print!("{}", build_prompt(&input, &ctx));
}
