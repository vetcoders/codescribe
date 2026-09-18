use std::io::{self, Read};

fn main() {
    let mut input = String::new();
    let result = io::stdin().take(1024 * 1024 + 1).read_to_string(&mut input);
    if result.is_err() || input.len() > 1024 * 1024 {
        eprintln!("structural AST input unreadable or exceeds 1 MiB");
        std::process::exit(2);
    }
    let request = match serde_json::from_str(&input) {
        Ok(request) => request,
        Err(error) => {
            eprintln!("structural AST malformed input: {error}");
            std::process::exit(2);
        }
    };
    let evidence = codescribe_structural_ast::analyze(request);
    match serde_json::to_string(&evidence) {
        Ok(output) => println!("{output}"),
        Err(error) => {
            eprintln!("structural AST serialization failed: {error}");
            std::process::exit(2);
        }
    }
}
