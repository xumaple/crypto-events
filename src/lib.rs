//! A simple payments engine for processing financial transactions.
//!
//! This crate provides a streaming transaction processor that handles deposits,
//! withdrawals, disputes, resolutions, and chargebacks. It reads transactions
//! from CSV input and outputs the final state of all client accounts.
//!
//! # Example
//!
//! ```no_run
//! use std::io::stdout;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     crypto_events::run("transactions.csv", stdout()).await
//! }
//! ```

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

mod client_account;
mod decimal;
mod engine;
mod transaction;
#[macro_use]
mod tracing;

pub use client_account::{ClientAccount, DisputeState, TransactionHistoryEntry};
pub use decimal::Decimal;
pub use engine::PaymentsEngine;
pub use transaction::{Transaction, TransactionType};

/// Type aliases for clarity.
pub type TransactionId = u32;
pub type ClientId = u16;

/// Run the payments engine on a CSV file and write results to a writer.
///
/// # Arguments
/// * `input_path` - Path to the input CSV file containing transactions
/// * `writer` - Writer to output the results to
///
/// # Returns
/// * `Ok(())` on success
/// * `Err` with description on failure
pub async fn run<P: AsRef<Path>, W: Write>(
    input_path: P,
    writer: W,
) -> Result<(), Box<dyn std::error::Error>> {
    let accounts = process_csv_file(input_path).await?;
    write_accounts_csv(accounts, writer)?;
    Ok(())
}

/// Process a CSV file through payments engine and return final account states.
async fn process_csv_file<P: AsRef<Path>>(
    input_path: P,
) -> Result<BTreeMap<ClientId, ClientAccount>, Box<dyn std::error::Error>> {
    let engine = PaymentsEngine::new();
    let sender = engine.sender();
    let engine_handle = engine.serve().await;

    // Read and parse transactions from CSV
    let mut reader = csv::ReaderBuilder::new()
        .trim(csv::Trim::All)
        .flexible(true)
        .from_path(input_path)?;

    for result in reader.deserialize() {
        match result {
            Ok(tx) => sender.send(tx).await?,
            Err(e) => error!("Failed to deserialize transaction: {}", e),
        }
    }

    // Close the channel to signal completion
    drop(sender);

    // Wait for the engine to finish processing
    let accounts = engine_handle.await?;
    Ok(accounts)
}

/// Write account states to a CSV writer.
fn write_accounts_csv<W: Write>(
    accounts: BTreeMap<ClientId, ClientAccount>,
    writer: W,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut csv_writer = csv::Writer::from_writer(writer);

    if accounts.is_empty() {
        // Write header manually when no accounts
        csv_writer.write_record(["client", "available", "held", "total", "locked"])?;
    }
    for account in accounts.values() {
        csv_writer.serialize(account)?;
    }

    csv_writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_account(client_id: ClientId, available: f64, held: f64, locked: bool) -> ClientAccount {
        let mut account = ClientAccount::new(client_id);
        account.available = Decimal::from_f64(available);
        account.held = Decimal::from_f64(held);
        account.total = Decimal::from_f64(available + held);
        account.locked = locked;
        account
    }

    // ========== write_accounts_csv Tests ==========

    #[test]
    fn test_write_accounts_csv_empty() {
        let accounts: BTreeMap<ClientId, ClientAccount> = BTreeMap::new();
        let mut output = Vec::new();

        write_accounts_csv(accounts, &mut output).unwrap();

        let output_str = String::from_utf8(output).unwrap();
        // Headers always written, even with no records
        assert_eq!(output_str, "client,available,held,total,locked\n");
    }

    #[test]
    fn test_write_accounts_csv_single_account() {
        let mut accounts = BTreeMap::new();
        accounts.insert(1, make_account(1, 100.0, 0.0, false));

        let mut output = Vec::new();
        write_accounts_csv(accounts, &mut output).unwrap();

        let output_str = String::from_utf8(output).unwrap();
        let lines: Vec<&str> = output_str.lines().collect();

        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "client,available,held,total,locked");
        assert_eq!(lines[1], "1,100.0,0.0,100.0,false");
    }

    #[test]
    fn test_write_accounts_csv_multiple_accounts_sorted() {
        let mut accounts = BTreeMap::new();
        // Insert in non-sorted order (BTreeMap will sort them)
        accounts.insert(3, make_account(3, 30.0, 0.0, false));
        accounts.insert(1, make_account(1, 10.0, 5.0, false));
        accounts.insert(2, make_account(2, 20.0, 0.0, true));

        let mut output = Vec::new();
        write_accounts_csv(accounts, &mut output).unwrap();

        let output_str = String::from_utf8(output).unwrap();
        let lines: Vec<&str> = output_str.lines().collect();

        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0], "client,available,held,total,locked");
        // Should be sorted by client ID
        assert!(lines[1].starts_with("1,"));
        assert!(lines[2].starts_with("2,"));
        assert!(lines[3].starts_with("3,"));
    }

    #[test]
    fn test_write_accounts_csv_locked_account() {
        let mut accounts = BTreeMap::new();
        accounts.insert(1, make_account(1, 50.0, 25.0, true));

        let mut output = Vec::new();
        write_accounts_csv(accounts, &mut output).unwrap();

        let output_str = String::from_utf8(output).unwrap();
        let lines: Vec<&str> = output_str.lines().collect();

        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "client,available,held,total,locked");
        assert_eq!(lines[1], "1,50.0,25.0,75.0,true");
    }

    #[test]
    fn test_write_accounts_csv_precision() {
        let mut accounts = BTreeMap::new();
        accounts.insert(1, make_account(1, 1.2345, 0.0001, false));

        let mut output = Vec::new();
        write_accounts_csv(accounts, &mut output).unwrap();

        let output_str = String::from_utf8(output).unwrap();
        let lines: Vec<&str> = output_str.lines().collect();

        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "client,available,held,total,locked");
        // Verify precision is maintained
        assert_eq!(lines[1], "1,1.2345,0.0001,1.2346,false");
    }

    #[test]
    fn test_write_accounts_csv_negative_balance() {
        let mut accounts = BTreeMap::new();
        accounts.insert(1, make_account(1, -50.0, 0.0, true));

        let mut output = Vec::new();
        write_accounts_csv(accounts, &mut output).unwrap();

        let output_str = String::from_utf8(output).unwrap();
        let lines: Vec<&str> = output_str.lines().collect();

        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "client,available,held,total,locked");
        assert_eq!(lines[1], "1,-50.0,0.0,-50.0,true");
    }
}
