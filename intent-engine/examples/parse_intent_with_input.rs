//! Validate LLM JSON + inject caller-known fields (raw_input, session_context, intent_id).
use cognos_intent_engine::{parse_llm_output_with_context, SessionContext};
use std::io::Read;

fn main() {
    let mut buf = String::new();
    std::io::stdin().read_to_string(&mut buf).expect("read stdin");
    let (user_input, json) = buf.split_once("\n---\n").expect("usage: user_input\\n---\\n{json}");
    let session = SessionContext {
        last_active_domain: None,
        last_active_files: vec![],
        current_time: "00:00".into(),
        time_since_last_session: None,
    };
    match parse_llm_output_with_context(json.trim(), user_input.trim(), &session) {
        Ok(schema) => println!("{}", serde_json::to_string(&schema).expect("serialize")),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}
