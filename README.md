# Crypto Events Engine

A simple payments engine that processes financial transactions from CSV input and outputs the final state of all client accounts.

## Overview

This application reads a stream of transactions (deposits, withdrawals, disputes, resolutions, and chargebacks) and maintains client account balances.

### Usage

```bash
cargo build --release
cargo run -- transactions.csv > accounts.csv
```

#### Documentation

```bash
cargo doc --open
```

#### Rust Version

This was built with `rustc 1.86.0`.

## Design Decisions

### Fixed-Point Decimal Arithmetic

For easy decimal arithmetic, my first thought was to use [`rust-decimal`](https://docs.rs/rust_decimal/latest/rust_decimal/). However, given that for our purposes we only need 4 degrees of precision and only addition/subtraction operations, I decided I didn't need something so complex (and which uses 96 bits!). Rather, I could get away with implementing my own `Decimal` struct using an internal i64 instead. This struct naively stores the true value × 10,000 rather than using floating-point. I have not benchmarked this, but I suspect that we trade precision for faster runtime here by using this approximation.

Values beyond 4 decimal places are rounded (not truncated). This struct still supports the basic `+`, `-`, `+=`, `-=` operators that regular integers use.

In using an internal i64, the maximum transaction total/amount is capped at `i64::MAX / 10000`, or about ~1.8 quadrillion. For this demo's purposes, this is reasonable; if needed, we can switch to `rust-decimal` or use an internal `i128`.

### Async Architecture

For the proper async architecture for this project, I debated between using `futures::Stream`s, `tokio::mpsc` channels, or `std::mpsc` channels. As `Stream`s are the async version of `Iterator`s, this made the most sense to me at first, especially given that I was reading out of a CSV into an iterator.

However, because this is meant to represent a holistic engine which supports many clients/transactions concurrently, yet still needs to handle everything chronologically, the "multi-producer" aspect of `mpsc` channels appealed, and I ultimately landed on the `tokio` version over `std` simply to allow for the multi-threaded feel even when this process is being run single-threaded. I believe that using `std::mpsc` channels could have been viable here as well.

By setting up the MPSC channel, this design helped me to:

- Decouple CSV parsing from transaction processing
- Enable future parallelization (multiple readers feeding one engine)
- Match the "asynchronous" nature of the problem at hand

### Memory pressure

The `Transaction` struct is by itself pretty lean. The CSV reader pulls transactions off of disk onto an iterator. Each row is read lazily, and the sender immediately sends the object off to its corresponding receiver, which is operating simultaneously off a separate thread.

The channel has a buffer of size 100 (chosen arbitrarily) to mitigate backpressure, but assuming the engine is able to handle requests adequately quickly, the effective memory usage of this design is O(1).

### Sorted Output

Client accounts are stored in a `BTreeMap` (rather than `HashMap`) to ensure deterministic output sorted by client ID, for easier testing. Assuming number of client accounts is not extremely large, the extra lookup runtime is negligible; `HashMap` could definitely also be used if needed.

### Tracing to stderr

Rather than using the `tracing` crate, and have to set up subscribers for a simple demo, I decided to emulate this by creating my own simple `tracing` module, which supports the `info!` and `error!` macros.

All error/info logs go to `stderr`, keeping `stdout` clean for CSV output. This follows Unix conventions and allows `cargo run -- input.csv > output.csv` to work correctly.

### Error Handling

Most of the errors involved in this project (outside of CSV read errors, etc) were to be silently ignored. Hence, we simply trace the error to `stderr` then continue handling the next request. Some of the errors we silently handle include:

- Transactions with negative amounts
- Transactions with empty amounts
- Forbidden transactions on locked accounts
- Duplicate transactions/disputes on the same tx ID

### Transaction Ledger

Each `ClientAccount` maintains a ledger of successful transactions. This enables dispute resolution by looking up the original transaction amount. Failed transactions (e.g., insufficient funds) are not recorded.

## Open Questions & Decisions

In creating this crate, some meta design questions came up. I've detailed these questions, and how I answered them, below:

1. ***Can withdrawals be disputed?*** This is an interesting question. In the real world, institutions usually see customers disputing withdrawals because they believe money was incorrectly taken from them (eg. unauthorized charge, double charge, etc.). However, because we are focused on catching fraud, we describe disputes as incorrect deposits rather than incorrect withdrawal. Given this baseline, I've decided to go with the simple version of this concept to only allow disputes for deposits.
2. ***How many transactions can a client dispute simultaneously?*** Based on real world institutions, I think it makes sense that multiple transactions can be simultaneously disputed. However, each transaction can only be disputed once total.
3. ***What transactions are allowed after an account is frozen?*** Presumably, after an account has been frozen due to a chargeback, we definitely cannot allow any more deposits or withdrawals. *Can the customer initiate more disputes?* I decided that after an account has been frozen, the customer cannot initiate any more disputes. However, we allow existing disputes (initiated before the freeze) to complete their resolution or chargeback. This prevents a chargeback from orphaning in-flight disputes.
4. ***Withdrawal limitations:*** Withdrawals cannot be negative. Overcharge withdrawals are ignored.
5. ***Withdrawing from a new account is an error which is ignored***, and thus does not create a new client account.
6. ***Failed withdrawals are not recorded in the ledger.*** This means that disputing a tx ID which links to a failed withdrawal will be considered invalid and ignored.
7. The spec says tx IDs are globally unique across clients. If we see a duplicate, we log an error and ignore it. This applies even if the first transaction failed (e.g., insufficient funds for withdrawal).
8. ***When are accounts created?*** Accounts are created lazily when processing a deposit or withdrawal. Dispute-related transactions for non-existent accounts are ignored.
9. ***0 amount deposits/withdrawals are allowed.*** It can even be disputed (though disputing $0 has no practical effect).

## Testing

The test suite contains 110 tests across unit and integration levels.

### Unit Tests by Module

| Module | Tests | Coverage |
|--------|-------|----------|
| `engine.rs` | 42 | End-to-end transaction processing through the `PaymentsEngine`. Tests the full flow from submitting sets of transactions to final account state, including multi-client scenarios, error detections, edge cases. This cuts out the step of input/output CSVs. |
| `client_account.rs` | 35 | Business logic for individual accounts. Tests balance updates, dispute state machine, locked account behavior, and ledger recording. |
| `decimal.rs` | 15 | Fixed-point arithmetic correctness. Tests serialization/deserialization roundtrips, display formatting, arithmetic operations (+, -, +=, -=), and precision handling for 5+ decimal places. |
| `lib.rs` | 6 | CSV output formatting. Tests header generation, field ordering, precision in output, and empty file handling. |
| `tracing.rs` | 3 | Logging macro correctness. |
| `transaction.rs` | 1 | Transaction type classification (`is_dispute_related`). |

### Integration Tests

There were fewer integration tests, here are them listed along with their purposes:

| Test | Purpose |
|------|---------|
| `test_basic_transactions` | Multi-client deposits and withdrawals from CSV |
| `test_dispute_resolve` | Full dispute → resolve flow via file input |
| `test_dispute_chargeback` | Full dispute → chargeback flow via file input |
| `test_empty_file` | Graceful handling of header-only input |
| `test_whitespace_handling` | CSV parsing with extra whitespace |
| `test_precision` | 4 decimal place accuracy through full pipeline |
| `test_duplicate_tx_id_ignored` | Duplicate transaction rejection |
| `test_output_sorted_by_client_id` | Deterministic ordering of output |

## Things I Didn't Do

The following are some considerations I had that did not go into this project.

### Generic Payments Engine

With a more complex scenario, perhaps we would have needed the `PaymentEngine` to be generic over some trait `T: Transaction`, or even some `A: Account`. But for our purposes, with transaction schema being very well defined, this was completely unnecessary.

### Multi-threaded transaction handling

In the real world, the payments engine is probably a distributed service handling transactions at large volumes. Given that this is being built at a much smaller scale, it's not worth the investment of chronologically ordering transactions for handling and hiding shared resources behind locks/primitives.

### Concrete Error Handling

If there were more things to do than just "silently ignore and continue", it would be very worth it to set up more concrete errors (eg. using `thiserror` crate) for better error handling.

## AI Use

Through the course of this project, AI was used for the following purposes:

- Writing unit tests, CSV shenanigans, and some trait impls (eg. Display, Deserialize). These were written by Claude Opus 4.5 via VSCode Copilot Agent mode. Claude also helped with debugging, summarizing, and writing parts of this README.
- Entire core infrastructure of the project was written by hand, but with Copilot autocomplete help.
- Consulted on some open questions about how real world institutions might approach such dilemmas.
