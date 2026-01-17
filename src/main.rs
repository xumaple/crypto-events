//! CLI entry point for the payments engine.
//!
//! Usage: `cargo run -- <transactions.csv>`

use std::env;
use std::process;

#[tokio::main]
async fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() != 2 {
        eprintln!("Usage: {} <transactions.csv>", args[0]);
        process::exit(1);
    }

    let input_path = &args[1];

    if let Err(e) = crypto_events::run(input_path, std::io::stdout()).await {
        eprintln!("Error: {}", e);
        process::exit(1);
    }
}
