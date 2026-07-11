//! Validate raw LLM JSON from stdin via `parse_llm_output`.
use cognos_intent_engine::schema_validator::parse_llm_output;
use std::io::Read;

fn main() {
    let mut raw = String::new();
    std::io::stdin()
        .read_to_string(&mut raw)
        .expect("read stdin");
    match parse_llm_output(raw.trim()) {
        Ok(schema) => {
            let json = serde_json::to_string(&schema).expect("serialize");
            println!("{json}");
        }
        Err(err) => {
            eprintln!("parse_llm_output: {err}");
            std::process::exit(1);
        }
    }
}
