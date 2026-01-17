//! The core payments processing engine.
//!
//! [`PaymentsEngine`] receives transactions via an async channel and maintains
//! the state of all client accounts.

use std::collections::{BTreeMap, HashSet};

use tokio::{
    sync::mpsc::{Receiver, Sender},
    task::JoinHandle,
};

use crate::client_account::ClientAccount;
use crate::transaction::Transaction;
use crate::{ClientId, TransactionId, error};

/// Payments processing engine.
pub struct PaymentsEngine {
    channel: (Sender<Transaction>, Receiver<Transaction>),
    processed_tx_ids: HashSet<TransactionId>,
}

impl Default for PaymentsEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl PaymentsEngine {
    pub fn new() -> Self {
        Self {
            channel: tokio::sync::mpsc::channel(100), // arbitrary buffer size
            processed_tx_ids: HashSet::new(),
        }
    }

    /// Get a sender to submit transactions to the engine.
    pub fn sender(&self) -> Sender<Transaction> {
        self.channel.0.clone()
    }

    /// Start processing transactions on a background task.
    ///
    /// Returns a JoinHandle that resolves to the final state of all client accounts.
    ///
    /// Client accounts are returned as a BTreeMap to maintain sorted order by ClientId.
    pub async fn serve(self) -> JoinHandle<BTreeMap<ClientId, ClientAccount>> {
        let mut receiver = self.channel.1;
        let mut processed_tx_ids = self.processed_tx_ids;
        tokio::spawn(async move {
            let mut accounts: BTreeMap<ClientId, ClientAccount> = BTreeMap::new();

            while let Some(tx) = receiver.recv().await {
                if tx.is_dispute_related() {
                    if let Some(account) = accounts.get_mut(&tx.client) {
                        account.adjudicate_claim(tx);
                    } else {
                        error!(
                            "Dispute-related transaction for non-existent account: {:?}",
                            tx
                        );
                    }
                } else if processed_tx_ids.insert(tx.tx) {
                    accounts
                        .entry(tx.client)
                        .or_insert_with(|| ClientAccount::new(tx.client))
                        .settle_transaction(tx);
                } else {
                    error!("Duplicate transaction ID received: {}", tx.tx);
                }
            }

            accounts
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Decimal, TransactionType};

    pub async fn process_transactions_vec(
        transactions: Vec<Transaction>,
    ) -> BTreeMap<ClientId, ClientAccount> {
        let engine = PaymentsEngine::new();
        let sender = engine.sender();
        let handle = engine.serve().await;
        for tx in transactions {
            sender.send(tx).await.unwrap();
        }
        drop(sender); // Close the channel
        handle.await.unwrap()
    }

    // ========== Helper Functions ==========

    fn deposit(client: ClientId, tx: TransactionId, amount: f64) -> Transaction {
        Transaction {
            transaction_type: TransactionType::Deposit,
            amount: Some(Decimal::from_f64(amount)),
            tx,
            client,
        }
    }

    fn withdrawal(client: ClientId, tx: TransactionId, amount: f64) -> Transaction {
        Transaction {
            transaction_type: TransactionType::Withdrawal,
            amount: Some(Decimal::from_f64(amount)),
            tx,
            client,
        }
    }

    fn dispute(client: ClientId, tx: TransactionId) -> Transaction {
        Transaction {
            transaction_type: TransactionType::Dispute,
            amount: None,
            tx,
            client,
        }
    }

    fn resolve(client: ClientId, tx: TransactionId) -> Transaction {
        Transaction {
            transaction_type: TransactionType::Resolve,
            amount: None,
            tx,
            client,
        }
    }

    fn chargeback(client: ClientId, tx: TransactionId) -> Transaction {
        Transaction {
            transaction_type: TransactionType::Chargeback,
            amount: None,
            tx,
            client,
        }
    }

    fn assert_account(
        accounts: &BTreeMap<ClientId, ClientAccount>,
        client_id: ClientId,
        available: f64,
        held: f64,
        total: f64,
        locked: bool,
    ) {
        let account = accounts.get(&client_id).expect("Client account not found");
        assert_eq!(
            account.available,
            Decimal::from_f64(available),
            "available mismatch for client {}",
            client_id
        );
        assert_eq!(
            account.held,
            Decimal::from_f64(held),
            "held mismatch for client {}",
            client_id
        );
        assert_eq!(
            account.total,
            Decimal::from_f64(total),
            "total mismatch for client {}",
            client_id
        );
        assert_eq!(
            account.locked, locked,
            "locked mismatch for client {}",
            client_id
        );
    }

    // ========== Basic Deposit/Withdrawal Tests ==========

    #[tokio::test]
    async fn test_empty_transactions() {
        let accounts = process_transactions_vec(vec![]).await;
        assert_eq!(accounts.len(), 0);
    }

    #[tokio::test]
    async fn test_single_deposit() {
        let accounts = process_transactions_vec(vec![deposit(1, 1, 10.0)]).await;
        assert_eq!(accounts.len(), 1);
        assert_account(&accounts, 1, 10.0, 0.0, 10.0, false);
    }

    #[tokio::test]
    async fn test_multiple_deposits_same_client() {
        let accounts = process_transactions_vec(vec![
            deposit(1, 1, 10.0),
            deposit(1, 2, 5.5),
            deposit(1, 3, 2.25),
        ])
        .await;
        assert_eq!(accounts.len(), 1);
        assert_account(&accounts, 1, 17.75, 0.0, 17.75, false);
    }

    #[tokio::test]
    async fn test_deposits_multiple_clients() {
        let accounts = process_transactions_vec(vec![
            deposit(1, 1, 10.0),
            deposit(2, 2, 20.0),
            deposit(1, 3, 5.0),
        ])
        .await;
        assert_eq!(accounts.len(), 2);
        assert_account(&accounts, 1, 15.0, 0.0, 15.0, false);
        assert_account(&accounts, 2, 20.0, 0.0, 20.0, false);
    }

    #[tokio::test]
    async fn test_deposit_precision() {
        let accounts = process_transactions_vec(vec![deposit(1, 1, 1.2345)]).await;
        assert_account(&accounts, 1, 1.2345, 0.0, 1.2345, false);
    }

    #[tokio::test]
    async fn test_withdrawal_sufficient_funds() {
        let accounts =
            process_transactions_vec(vec![deposit(1, 1, 10.0), withdrawal(1, 2, 4.0)]).await;
        assert_account(&accounts, 1, 6.0, 0.0, 6.0, false);
    }

    #[tokio::test]
    async fn test_withdrawal_exact_balance() {
        let accounts =
            process_transactions_vec(vec![deposit(1, 1, 10.0), withdrawal(1, 2, 10.0)]).await;
        assert_account(&accounts, 1, 0.0, 0.0, 0.0, false);
    }

    #[tokio::test]
    async fn test_withdrawal_insufficient_funds_ignored() {
        let accounts =
            process_transactions_vec(vec![deposit(1, 1, 10.0), withdrawal(1, 2, 15.0)]).await;
        assert_account(&accounts, 1, 10.0, 0.0, 10.0, false);
    }

    #[tokio::test]
    async fn test_withdrawal_no_account() {
        let accounts = process_transactions_vec(vec![withdrawal(1, 1, 5.0)]).await;
        assert_account(&accounts, 1, 0.0, 0.0, 0.0, false);
    }

    // ========== Dispute Flow Tests ==========

    #[tokio::test]
    async fn test_dispute_holds_funds() {
        let accounts = process_transactions_vec(vec![deposit(1, 1, 10.0), dispute(1, 1)]).await;
        // available decreases, held increases, total unchanged
        assert_account(&accounts, 1, 0.0, 10.0, 10.0, false);
    }

    #[tokio::test]
    async fn test_dispute_then_resolve_releases_funds() {
        let accounts =
            process_transactions_vec(vec![deposit(1, 1, 10.0), dispute(1, 1), resolve(1, 1)]).await;
        assert_account(&accounts, 1, 10.0, 0.0, 10.0, false);
    }

    #[tokio::test]
    async fn test_dispute_then_chargeback_removes_funds_and_locks() {
        let accounts =
            process_transactions_vec(vec![deposit(1, 1, 10.0), dispute(1, 1), chargeback(1, 1)])
                .await;
        assert_account(&accounts, 1, 0.0, 0.0, 0.0, true);
    }

    #[tokio::test]
    async fn test_dispute_partial_balance() {
        // Dispute one of multiple deposits
        let accounts =
            process_transactions_vec(vec![deposit(1, 1, 10.0), deposit(1, 2, 5.0), dispute(1, 1)])
                .await;
        assert_account(&accounts, 1, 5.0, 10.0, 15.0, false);
    }

    #[tokio::test]
    async fn test_chargeback_partial_balance() {
        let accounts = process_transactions_vec(vec![
            deposit(1, 1, 10.0),
            deposit(1, 2, 5.0),
            dispute(1, 1),
            chargeback(1, 1),
        ])
        .await;
        // tx 2 remains, account locked
        assert_account(&accounts, 1, 5.0, 0.0, 5.0, true);
    }

    #[tokio::test]
    async fn test_dispute_already_disputed_ignored() {
        let accounts = process_transactions_vec(vec![
            deposit(1, 1, 10.0),
            dispute(1, 1),
            dispute(1, 1), // Second dispute - should be ignored
        ])
        .await;
        // Should NOT double-hold
        assert_account(&accounts, 1, 0.0, 10.0, 10.0, false);
    }

    #[tokio::test]
    async fn test_redispute_after_resolve_ignored() {
        // Policy: do NOT allow re-disputing a previously resolved transaction
        let accounts = process_transactions_vec(vec![
            deposit(1, 1, 10.0),
            dispute(1, 1),
            resolve(1, 1),
            dispute(1, 1), // Should be ignored - already resolved
        ])
        .await;
        // Funds should remain available (not re-held)
        assert_account(&accounts, 1, 10.0, 0.0, 10.0, false);
    }

    // ========== Invalid Operation Tests ==========

    #[tokio::test]
    async fn test_dispute_nonexistent_tx_ignored() {
        let accounts = process_transactions_vec(vec![deposit(1, 1, 10.0), dispute(1, 999)]).await;
        assert_account(&accounts, 1, 10.0, 0.0, 10.0, false);
    }

    #[tokio::test]
    async fn test_resolve_nonexistent_tx_ignored() {
        let accounts = process_transactions_vec(vec![deposit(1, 1, 10.0), resolve(1, 999)]).await;
        assert_account(&accounts, 1, 10.0, 0.0, 10.0, false);
    }

    #[tokio::test]
    async fn test_chargeback_nonexistent_tx_ignored() {
        let accounts =
            process_transactions_vec(vec![deposit(1, 1, 10.0), chargeback(1, 999)]).await;
        assert_account(&accounts, 1, 10.0, 0.0, 10.0, false);
    }

    #[tokio::test]
    async fn test_dispute_wrong_client_ignored() {
        let accounts = process_transactions_vec(vec![
            deposit(1, 1, 10.0),
            deposit(2, 2, 20.0),
            dispute(2, 1), // Client 2 tries to dispute client 1's tx
        ])
        .await;
        assert_eq!(accounts.len(), 2);
        assert_account(&accounts, 1, 10.0, 0.0, 10.0, false);
        assert_account(&accounts, 2, 20.0, 0.0, 20.0, false);
    }

    #[tokio::test]
    async fn test_resolve_wrong_client_ignored() {
        let accounts = process_transactions_vec(vec![
            deposit(1, 1, 10.0),
            dispute(1, 1),
            resolve(2, 1), // Wrong client
        ])
        .await;
        assert_eq!(accounts.len(), 1); // Client 2 should NOT be created
        assert_account(&accounts, 1, 0.0, 10.0, 10.0, false);
    }

    #[tokio::test]
    async fn test_chargeback_wrong_client_ignored() {
        let accounts = process_transactions_vec(vec![
            deposit(1, 1, 10.0),
            dispute(1, 1),
            chargeback(2, 1), // Wrong client
        ])
        .await;
        assert_eq!(accounts.len(), 1); // Client 2 should NOT be created
        assert_account(&accounts, 1, 0.0, 10.0, 10.0, false);
    }

    #[tokio::test]
    async fn test_dispute_withdrawal_ignored() {
        let accounts = process_transactions_vec(vec![
            deposit(1, 1, 10.0),
            withdrawal(1, 2, 3.0),
            dispute(1, 2), // Can't dispute a withdrawal
        ])
        .await;
        assert_account(&accounts, 1, 7.0, 0.0, 7.0, false);
    }

    #[tokio::test]
    async fn test_resolve_not_disputed_ignored() {
        let accounts = process_transactions_vec(vec![
            deposit(1, 1, 10.0),
            resolve(1, 1), // tx 1 not under dispute
        ])
        .await;
        assert_account(&accounts, 1, 10.0, 0.0, 10.0, false);
    }

    #[tokio::test]
    async fn test_chargeback_not_disputed_ignored() {
        let accounts = process_transactions_vec(vec![
            deposit(1, 1, 10.0),
            chargeback(1, 1), // tx 1 not under dispute
        ])
        .await;
        assert_account(&accounts, 1, 10.0, 0.0, 10.0, false);
    }

    #[tokio::test]
    async fn test_operations_before_any_deposit_ignored() {
        // All should be silently ignored - no accounts created
        assert_eq!(process_transactions_vec(vec![dispute(1, 1)]).await.len(), 0);
        assert_eq!(process_transactions_vec(vec![resolve(1, 1)]).await.len(), 0);
        assert_eq!(
            process_transactions_vec(vec![chargeback(1, 1)]).await.len(),
            0
        );
    }

    // ========== Locked Account Tests ==========

    #[tokio::test]
    async fn test_locked_account_rejects_deposit() {
        let accounts = process_transactions_vec(vec![
            deposit(1, 1, 10.0),
            dispute(1, 1),
            chargeback(1, 1),
            deposit(1, 2, 5.0), // Should be ignored
        ])
        .await;
        assert_account(&accounts, 1, 0.0, 0.0, 0.0, true);
    }

    #[tokio::test]
    async fn test_locked_account_rejects_withdrawal() {
        let accounts = process_transactions_vec(vec![
            deposit(1, 1, 10.0),
            deposit(1, 2, 10.0),
            dispute(1, 1),
            chargeback(1, 1),
            withdrawal(1, 3, 5.0), // Should be ignored
        ])
        .await;
        assert_account(&accounts, 1, 10.0, 0.0, 10.0, true);
    }

    #[tokio::test]
    async fn test_locked_account_rejects_dispute() {
        let accounts = process_transactions_vec(vec![
            deposit(1, 1, 10.0),
            deposit(1, 2, 5.0),
            dispute(1, 1),
            chargeback(1, 1),
            dispute(1, 2), // Should be ignored
        ])
        .await;
        // tx 2 should NOT be disputed
        assert_account(&accounts, 1, 5.0, 0.0, 5.0, true);
    }

    #[tokio::test]
    async fn test_locked_account_rejects_resolve() {
        // This tests that resolve on a NEW dispute (after freeze) is rejected.
        // But pre-freeze disputes CAN be resolved - see test_multiple_disputes_before_freeze_can_complete_after
        let accounts = process_transactions_vec(vec![
            deposit(1, 1, 10.0),
            deposit(1, 2, 5.0),
            dispute(1, 2),    // Pre-freeze dispute
            dispute(1, 1),    // Pre-freeze dispute
            chargeback(1, 1), // Freezes account, total: 5
            resolve(1, 2),    // Pre-freeze dispute - ALLOWED
        ])
        .await;
        // tx 2 resolved: available = 5, held = 0
        assert_account(&accounts, 1, 5.0, 0.0, 5.0, true);
    }

    #[tokio::test]
    async fn test_locked_account_rejects_chargeback() {
        // This tests that chargeback on pre-freeze disputes still works
        let accounts = process_transactions_vec(vec![
            deposit(1, 1, 10.0),
            deposit(1, 2, 5.0),
            dispute(1, 2),    // Pre-freeze dispute
            dispute(1, 1),    // Pre-freeze dispute
            chargeback(1, 1), // Freezes account, total: 5
            chargeback(1, 2), // Pre-freeze dispute - ALLOWED, total: 0
        ])
        .await;
        // tx 2 also charged back
        assert_account(&accounts, 1, 0.0, 0.0, 0.0, true);
    }

    // ========== Duplicate Transaction ID Tests ==========

    #[tokio::test]
    async fn test_duplicate_tx_id_same_client_ignored() {
        let accounts = process_transactions_vec(vec![
            deposit(1, 1, 10.0),
            deposit(1, 1, 50.0), // Duplicate - ignored
        ])
        .await;
        assert_account(&accounts, 1, 10.0, 0.0, 10.0, false);
    }

    #[tokio::test]
    async fn test_duplicate_tx_id_different_client_ignored() {
        // tx IDs are globally unique
        let accounts = process_transactions_vec(vec![
            deposit(1, 1, 10.0),
            deposit(2, 1, 20.0), // Same tx id - ignored
        ])
        .await;
        assert_eq!(accounts.len(), 1);
        assert_account(&accounts, 1, 10.0, 0.0, 10.0, false);
    }

    // ========== Negative Balance Edge Cases ==========

    #[tokio::test]
    async fn test_dispute_after_partial_spend() {
        // Dispute a deposit after some funds were already withdrawn
        // This causes available to go negative (client owes money during dispute)
        let accounts = process_transactions_vec(vec![
            deposit(1, 1, 10.0),
            withdrawal(1, 2, 7.0),
            dispute(1, 1),
        ])
        .await;
        assert_account(&accounts, 1, -7.0, 10.0, 3.0, false);
    }

    #[tokio::test]
    async fn test_resolve_after_partial_spend() {
        let accounts = process_transactions_vec(vec![
            deposit(1, 1, 10.0),
            withdrawal(1, 2, 7.0),
            dispute(1, 1),
            resolve(1, 1),
        ])
        .await;
        // Back to normal
        assert_account(&accounts, 1, 3.0, 0.0, 3.0, false);
    }

    #[tokio::test]
    async fn test_chargeback_after_partial_spend() {
        // Chargeback after funds spent = client owes money
        let accounts = process_transactions_vec(vec![
            deposit(1, 1, 10.0),
            withdrawal(1, 2, 7.0),
            dispute(1, 1),
            chargeback(1, 1),
        ])
        .await;
        assert_account(&accounts, 1, -7.0, 0.0, -7.0, true);
    }

    // ========== Complex Scenario ==========

    #[tokio::test]
    async fn test_complex_multi_client_scenario() {
        let accounts = process_transactions_vec(vec![
            deposit(1, 1, 100.0),
            deposit(2, 2, 200.0),
            withdrawal(1, 3, 25.0),
            deposit(1, 4, 50.0),
            dispute(1, 1),
            deposit(2, 5, 50.0),
            resolve(1, 1),
            withdrawal(2, 6, 100.0),
            dispute(2, 2),
            chargeback(2, 2),
            deposit(2, 7, 1000.0), // Ignored - locked
        ])
        .await;
        assert_eq!(accounts.len(), 2);
        // Client 1: 100 - 25 + 50 = 125
        assert_account(&accounts, 1, 125.0, 0.0, 125.0, false);
        // Client 2: 200 + 50 - 100 - 200(chargeback) = -50
        assert_account(&accounts, 2, -50.0, 0.0, -50.0, true);
    }

    #[tokio::test]
    async fn test_multiple_disputes_before_freeze_can_complete_after() {
        // Scenario: Multiple disputes initiated before account freeze.
        // After one chargeback freezes the account:
        // - NEW disputes should be rejected
        // - EXISTING disputes (initiated before freeze) can still resolve/chargeback
        let accounts = process_transactions_vec(vec![
            // Setup: 4 deposits
            deposit(1, 1, 100.0), // Will be disputed and charged back (freezes account)
            deposit(1, 2, 50.0),  // Will be disputed before freeze, resolved after
            deposit(1, 3, 25.0),  // Will be disputed before freeze, charged back after
            deposit(1, 4, 75.0),  // Will NOT be disputed before freeze
            // Initiate disputes on tx 1, 2, 3 BEFORE any freeze
            dispute(1, 1), // available: 150, held: 100
            dispute(1, 2), // available: 100, held: 150
            dispute(1, 3), // available: 75, held: 175
            // Chargeback tx 1 - this freezes the account
            chargeback(1, 1), // held: 75, total: 150, locked: true
            // Try to dispute tx 4 AFTER freeze - should be rejected
            dispute(1, 4),
            // Resolve tx 2 - should succeed (dispute was initiated before freeze)
            resolve(1, 2), // held: 25, available increases by 50
            // Chargeback tx 3 - should succeed (dispute was initiated before freeze)
            chargeback(1, 3), // held: 0, total decreases by 25
        ])
        .await;

        // Final state:
        // - tx 1: charged back (-100 from total)
        // - tx 2: resolved (back to available)
        // - tx 3: charged back (-25 from total)
        // - tx 4: never disputed (still in available... wait, account is locked)
        //
        // Initial total: 100 + 50 + 25 + 75 = 250
        // After tx 1 chargeback: total = 150
        // After tx 3 chargeback: total = 125
        // available = 75 (tx 4, undisputed) + 50 (tx 2, resolved) = 125
        // held = 0 (all disputes resolved or charged back)
        assert_account(&accounts, 1, 125.0, 0.0, 125.0, true);
    }

    // ========== Double Resolution/Chargeback Tests ==========
    // These tests are designed so that incorrect behavior produces DIFFERENT results
    // from correct behavior, ensuring bugs are actually caught.

    #[tokio::test]
    async fn test_chargeback_already_chargedback_ignored() {
        // Use multiple disputes so we can chargeback twice on a locked account.
        // This tests "already chargedback" rejection, not just "account locked" rejection.
        let accounts = process_transactions_vec(vec![
            deposit(1, 1, 10.0),
            deposit(1, 2, 20.0),
            deposit(1, 3, 30.0),
            // Dispute two transactions BEFORE any freeze
            dispute(1, 1),
            dispute(1, 2),
            // First chargeback freezes account
            chargeback(1, 1), // total: 60 - 10 = 50
            // Second chargeback on different tx (pre-freeze dispute) should work
            chargeback(1, 2), // total: 50 - 20 = 30
            // Third chargeback on tx 1 again - should be IGNORED (already charged back)
            chargeback(1, 1), // If buggy: total = 20. Correct: total = 30
        ])
        .await;
        // If code allows double chargeback, total would be 20 instead of 30
        assert_account(&accounts, 1, 30.0, 0.0, 30.0, true);
    }

    #[tokio::test]
    async fn test_resolve_already_chargedback_ignored() {
        // Similar setup: multiple pre-freeze disputes so we can test resolve after chargeback
        let accounts = process_transactions_vec(vec![
            deposit(1, 1, 10.0),
            deposit(1, 2, 20.0),
            // Dispute both BEFORE freeze
            dispute(1, 1),
            dispute(1, 2),
            // Chargeback tx 1 (freezes account)
            chargeback(1, 1), // available: 0, held: 20, total: 20
            // Resolve tx 2 (pre-freeze dispute, should work)
            resolve(1, 2), // available: 20, held: 0, total: 20
            // Try to resolve tx 1 (already charged back) - should be IGNORED
            resolve(1, 1), // If buggy (adds 10 back): available = 30. Correct: 20
        ])
        .await;
        // If code allows resolve after chargeback, available would be 30 instead of 20
        assert_account(&accounts, 1, 20.0, 0.0, 20.0, true);
    }

    #[tokio::test]
    async fn test_resolve_already_resolved_ignored() {
        // Use additional funds so double-resolve would show a different balance
        let accounts = process_transactions_vec(vec![
            deposit(1, 1, 10.0),
            deposit(1, 2, 20.0),
            dispute(1, 1), // available: 20, held: 10
            resolve(1, 1), // available: 30, held: 0
            resolve(1, 1), // If buggy (adds 10 again): available = 40. Correct: 30
        ])
        .await;
        // If code uses stored tx amount instead of checking state, available would be 40
        assert_account(&accounts, 1, 30.0, 0.0, 30.0, false);
    }

    #[tokio::test]
    async fn test_chargeback_already_resolved_ignored() {
        // After resolve, chargeback should not work (tx is no longer under dispute)
        let accounts = process_transactions_vec(vec![
            deposit(1, 1, 10.0),
            deposit(1, 2, 20.0),
            dispute(1, 1),
            resolve(1, 1),    // available: 30, held: 0, total: 30
            chargeback(1, 1), // If buggy: total = 20, locked = true. Correct: 30, unlocked
        ])
        .await;
        // If code allows chargeback after resolve, total would be 20 and account locked
        assert_account(&accounts, 1, 30.0, 0.0, 30.0, false);
    }
}
