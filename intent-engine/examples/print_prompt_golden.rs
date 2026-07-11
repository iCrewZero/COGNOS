//! Emit `build_prompt(input, context)` for a golden fixture path or stdin JSON.
use cognos_intent_engine::prompt::build_prompt;
use cognos_intent_engine::schema_validator::SessionContext;
use std::io::Read;

fn main() {
    let arg = std::env::args().nth(1);
    let raw = if let Some(path) = arg {
        std::fs::read_to_string(&path).unwrap_or_else(|e| {
            eprintln!("cannot read {path}: {e}");
            std::process::exit(1);
        })
    } else {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .expect("read stdin");
        buf
    };

    let value: serde_json::Value =
        serde_json::from_str(&raw).expect("golden JSON must parse");
    let input = value["input"]
        .as_str()
        .expect("missing string `input`")
        .to_string();
    let context: SessionContext =
        serde_json::from_value(value["context"].clone()).expect("invalid `context`");

    print!("{}", build_prompt(&input, &context));
}
