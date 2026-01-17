//! Integration tests for the payments engine CSV processing.

use std::path::PathBuf;

/// Get path to test input file.
fn test_input(filename: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("inputs")
        .join(filename)
}

/// Run the engine on a test file and return the output as a string.
async fn run_and_capture(filename: &str) -> String {
    let mut output = Vec::new();
    crypto_events::run(test_input(filename), &mut output)
        .await
        .expect("run should succeed");
    String::from_utf8(output).expect("output should be valid UTF-8")
}

// ========== Integration Tests ==========

#[tokio::test]
async fn test_basic_transactions() {
    let output = run_and_capture("basic_transactions.csv").await;

    // Client 1: 10.0 - 5.0 + 3.5 = 8.5
    // Client 2: 20.0
    assert_eq!(
        output,
        "client,available,held,total,locked\n\
         1,8.5,0.0,8.5,false\n\
         2,20.0,0.0,20.0,false\n"
    );
}

#[tokio::test]
async fn test_dispute_resolve() {
    let output = run_and_capture("dispute_resolve.csv").await;

    // Deposit 100, dispute, resolve -> back to available
    assert_eq!(
        output,
        "client,available,held,total,locked\n\
         1,100.0,0.0,100.0,false\n"
    );
}

#[tokio::test]
async fn test_dispute_chargeback() {
    let output = run_and_capture("dispute_chargeback.csv").await;

    // Deposit 100 and 50, dispute first, chargeback -> only 50 remains, locked
    assert_eq!(
        output,
        "client,available,held,total,locked\n\
         1,50.0,0.0,50.0,true\n"
    );
}

#[tokio::test]
async fn test_empty_file() {
    let output = run_and_capture("empty.csv").await;

    // Headers always written, even with no records
    assert_eq!(output, "client,available,held,total,locked\n");
}

#[tokio::test]
async fn test_whitespace_handling() {
    let output = run_and_capture("whitespace.csv").await;

    // Client 1: 10.0 - 5.0 = 5.0
    // Client 2: 20.0
    assert_eq!(
        output,
        "client,available,held,total,locked\n\
         1,5.0,0.0,5.0,false\n\
         2,20.0,0.0,20.0,false\n"
    );
}

#[tokio::test]
async fn test_precision() {
    let output = run_and_capture("precision.csv").await;

    // 1.2345 + 0.0001 - 0.1234 = 1.1112
    assert_eq!(
        output,
        "client,available,held,total,locked\n\
         1,1.1112,0.0,1.1112,false\n"
    );
}

#[tokio::test]
async fn test_duplicate_tx_id_ignored() {
    let output = run_and_capture("duplicate_tx.csv").await;

    // Client 2's deposit with tx 1 is ignored (duplicate global tx ID)
    // Client 1: 100.0 + 25.0 = 125.0
    assert_eq!(
        output,
        "client,available,held,total,locked\n\
         1,125.0,0.0,125.0,false\n"
    );
}

#[tokio::test]
async fn test_output_sorted_by_client_id() {
    // basic_transactions.csv creates client 1 first, then client 2
    // Output should be sorted by client ID regardless of creation order
    let output = run_and_capture("basic_transactions.csv").await;

    let lines: Vec<&str> = output.lines().collect();
    assert!(lines[1].starts_with("1,"));
    assert!(lines[2].starts_with("2,"));
}
