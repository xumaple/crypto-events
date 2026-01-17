//! Client account management and transaction settlement.
//!
//! [`ClientAccount`] tracks a single client's balances (available, held, total)
//! and handles the business logic for deposits, withdrawals, and dispute resolution.

use serde::Serialize;
use std::collections::HashMap;

use crate::{
    ClientId, TransactionId,
    decimal::Decimal,
    error,
    transaction::{Transaction, TransactionType},
};

/// Dispute state for a transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisputeState {
    Disputed,
    Resolved,
    ChargedBack,
}

/// Entry in [`ClientAccount`]'s transaction history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransactionHistoryEntry {
    pub transaction_type: TransactionType,
    pub amount: Decimal,
}

impl TryFrom<Transaction> for TransactionHistoryEntry {
    type Error = (); // Could add some error type here

    fn try_from(tx: Transaction) -> Result<Self, Self::Error> {
        if tx.is_dispute_related() {
            return Err(());
        }
        tx.amount.ok_or(()).map(|amount| Self {
            transaction_type: tx.transaction_type,
            amount,
        })
    }
}

/// Client account state.
///
/// Maintains the invariant: `total = available + held`
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct ClientAccount {
    #[serde(rename = "client")]
    pub client_id: ClientId,
    pub available: Decimal,
    pub held: Decimal,
    pub total: Decimal,
    /// Transactions currently under dispute.
    #[serde(skip)]
    disputes: HashMap<TransactionId, DisputeState>,
    /// Records of completed fund transfers (deposits/withdrawals).
    #[serde(skip)]
    ledger: HashMap<TransactionId, TransactionHistoryEntry>,
    pub locked: bool,
}

impl ClientAccount {
    /// Create a new client account with zero balances.
    pub fn new(client_id: ClientId) -> Self {
        Self {
            client_id,
            available: Decimal::default(),
            held: Decimal::default(),
            total: Decimal::default(),
            disputes: HashMap::new(),
            ledger: HashMap::new(),
            locked: false,
        }
    }

    /// Settle a deposit or withdrawal transaction.
    ///
    /// Updates available and total balances accordingly. The transaction is
    /// recorded in the ledger only if successful (for potential future disputes).
    ///
    /// # Ignored cases (logged as errors)
    /// - Locked accounts
    /// - Missing or negative amounts
    /// - Insufficient funds for withdrawals
    pub fn settle_transaction(&mut self, tx: Transaction) {
        if self.locked {
            return; // Ignore all transactions on locked accounts
        }

        // Validate amount is present and non-negative, otherwise log error and ignore
        let amount = match tx.amount {
            Some(amt) if amt >= Decimal::default() => amt,
            Some(_) => {
                error!("Rejecting transaction with negative amount: {:?}", tx);
                return;
            }
            None => {
                error!("Found malformed transaction entry: {:?}", tx);
                return;
            }
        };

        match tx.transaction_type {
            TransactionType::Deposit => {
                self.available += amount;
                self.total += amount;
            }
            TransactionType::Withdrawal => {
                if self.available >= amount {
                    self.available -= amount;
                    self.total -= amount;
                } else {
                    return; // Don't record failed withdrawals
                }
            }
            TransactionType::Dispute | TransactionType::Resolve | TransactionType::Chargeback => {
                unreachable!()
            }
        }

        // SAFETY: This only fails for dispute-related transactions or if amount is None.
        //         Neither of these cases reach here due to earlier checks.
        self.ledger
            .insert(tx.tx, TransactionHistoryEntry::try_from(tx).unwrap());
    }

    /// Adjudicate a dispute claim (dispute, resolve, or chargeback).
    ///
    /// # Design Decision: Pre-freeze disputes can still be resolved/charged back
    ///
    /// When an account is locked (frozen) after a chargeback, we reject NEW disputes
    /// but allow existing disputes that were initiated before the freeze to be resolved
    /// or charged back.
    pub fn adjudicate_claim(&mut self, tx: Transaction) {
        if let Some(ledger_entry) = self.ledger.get(&tx.tx) {
            match tx.transaction_type {
                TransactionType::Dispute => {
                    if self.locked {
                        error!(
                            "Received new dispute on locked account {}: {:?}",
                            self.client_id, tx
                        );
                        return; // Reject NEW disputes on locked accounts
                    }
                    if self.disputes.contains_key(&tx.tx) {
                        error!("Received duplicate dispute for transaction: {:?}", tx);
                        return; // Already disputed (or resolved/chargebacked)
                    }
                    // Only deposits can be disputed
                    if ledger_entry.transaction_type == TransactionType::Deposit {
                        self.available -= ledger_entry.amount;
                        self.held += ledger_entry.amount;
                        self.disputes.insert(tx.tx, DisputeState::Disputed);
                    } else {
                        error!(
                            "Received request to dispute withdrawal transaction: {:?}",
                            tx
                        );
                    }
                }
                TransactionType::Resolve => {
                    if let Some(state) = self.disputes.get_mut(&tx.tx) {
                        if *state == DisputeState::Disputed {
                            self.held -= ledger_entry.amount;
                            self.available += ledger_entry.amount;
                            *state = DisputeState::Resolved;
                        } else {
                            error!(
                                "Received request to resolve non-disputed transaction: {:?}",
                                tx
                            );
                        }
                    } else {
                        error!("Received request to resolve unknown transaction: {:?}", tx);
                    }
                }
                TransactionType::Chargeback => {
                    if let Some(state) = self.disputes.get_mut(&tx.tx) {
                        if *state == DisputeState::Disputed {
                            self.held -= ledger_entry.amount;
                            self.total -= ledger_entry.amount;
                            self.locked = true;
                            *state = DisputeState::ChargedBack;
                        } else {
                            error!(
                                "Received request to chargeback non-disputed transaction: {:?}",
                                tx
                            );
                        }
                    } else {
                        error!(
                            "Received request to chargeback unknown transaction: {:?}",
                            tx
                        );
                    }
                }
                TransactionType::Deposit | TransactionType::Withdrawal => {}
            }
        } else {
            error!(
                "Received dispute-related request for unknown transaction: {:?}",
                tx
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========== Test Helpers ==========

    fn make_deposit(tx: TransactionId, amount: f64) -> Transaction {
        Transaction {
            transaction_type: TransactionType::Deposit,
            amount: Some(Decimal::from_f64(amount)),
            tx,
            client: 1,
        }
    }

    fn make_withdrawal(tx: TransactionId, amount: f64) -> Transaction {
        Transaction {
            transaction_type: TransactionType::Withdrawal,
            amount: Some(Decimal::from_f64(amount)),
            tx,
            client: 1,
        }
    }

    fn make_dispute(tx: TransactionId) -> Transaction {
        Transaction {
            transaction_type: TransactionType::Dispute,
            amount: None,
            tx,
            client: 1,
        }
    }

    fn make_resolve(tx: TransactionId) -> Transaction {
        Transaction {
            transaction_type: TransactionType::Resolve,
            amount: None,
            tx,
            client: 1,
        }
    }

    fn make_chargeback(tx: TransactionId) -> Transaction {
        Transaction {
            transaction_type: TransactionType::Chargeback,
            amount: None,
            tx,
            client: 1,
        }
    }

    fn assert_balances(account: &ClientAccount, available: f64, held: f64, total: f64) {
        assert_eq!(
            account.available,
            Decimal::from_f64(available),
            "available mismatch"
        );
        assert_eq!(account.held, Decimal::from_f64(held), "held mismatch");
        assert_eq!(account.total, Decimal::from_f64(total), "total mismatch");
    }

    // ========== settle_transaction Tests ==========

    #[test]
    fn test_deposit_updates_available_and_total() {
        let mut account = ClientAccount::new(1);
        account.settle_transaction(make_deposit(1, 100.0));

        assert_balances(&account, 100.0, 0.0, 100.0);
        assert!(!account.locked);
    }

    #[test]
    fn test_deposit_records_ledger() {
        let mut account = ClientAccount::new(1);
        account.settle_transaction(make_deposit(1, 100.0));

        assert!(account.ledger.contains_key(&1));
        let entry = account.ledger.get(&1).unwrap();
        assert_eq!(entry.transaction_type, TransactionType::Deposit);
        assert_eq!(entry.amount, Decimal::from_f64(100.0));
    }

    #[test]
    fn test_deposit_with_none_amount_ignored() {
        let mut account = ClientAccount::new(1);
        let tx = Transaction {
            transaction_type: TransactionType::Deposit,
            amount: None,
            tx: 1,
            client: 1,
        };
        account.settle_transaction(tx);

        assert_balances(&account, 0.0, 0.0, 0.0);
        // Ledger should NOT be recorded for None amount
        assert!(!account.ledger.contains_key(&1));
    }

    #[test]
    fn test_withdrawal_sufficient_funds() {
        let mut account = ClientAccount::new(1);
        account.settle_transaction(make_deposit(1, 100.0));
        account.settle_transaction(make_withdrawal(2, 30.0));

        assert_balances(&account, 70.0, 0.0, 70.0);
    }

    #[test]
    fn test_withdrawal_insufficient_funds_rejected() {
        let mut account = ClientAccount::new(1);
        account.settle_transaction(make_deposit(1, 100.0));
        account.settle_transaction(make_withdrawal(2, 150.0));

        // Balance unchanged
        assert_balances(&account, 100.0, 0.0, 100.0);
        // Failed withdrawal NOT recorded in ledger (can't dispute what didn't happen)
        assert!(!account.ledger.contains_key(&2));
    }

    #[test]
    fn test_withdrawal_exact_balance() {
        let mut account = ClientAccount::new(1);
        account.settle_transaction(make_deposit(1, 100.0));
        account.settle_transaction(make_withdrawal(2, 100.0));

        assert_balances(&account, 0.0, 0.0, 0.0);
    }

    #[test]
    fn test_withdrawal_with_none_amount_ignored() {
        let mut account = ClientAccount::new(1);
        account.settle_transaction(make_deposit(1, 100.0));
        let tx = Transaction {
            transaction_type: TransactionType::Withdrawal,
            amount: None,
            tx: 2,
            client: 1,
        };
        account.settle_transaction(tx);

        assert_balances(&account, 100.0, 0.0, 100.0);
    }

    #[test]
    fn test_settle_transaction_on_locked_account_rejected() {
        let mut account = ClientAccount::new(1);
        account.locked = true;
        account.settle_transaction(make_deposit(1, 100.0));

        assert_balances(&account, 0.0, 0.0, 0.0);
        assert!(!account.ledger.contains_key(&1));
    }

    #[test]
    fn test_duplicate_tx_id_overwrites_ledger() {
        // Note: Engine prevents this, but at ClientAccount level it overwrites
        let mut account = ClientAccount::new(1);
        account.settle_transaction(make_deposit(1, 100.0));
        account.settle_transaction(make_deposit(1, 50.0)); // Same tx ID

        // Both deposits apply (ClientAccount doesn't check for duplicates)
        assert_balances(&account, 150.0, 0.0, 150.0);
        // Ledger has the second entry
        let entry = account.ledger.get(&1).unwrap();
        assert_eq!(entry.amount, Decimal::from_f64(50.0));
    }

    // ========== adjudicate_claim Tests ==========

    #[test]
    fn test_dispute_holds_funds() {
        let mut account = ClientAccount::new(1);
        account.settle_transaction(make_deposit(1, 100.0));
        account.adjudicate_claim(make_dispute(1));

        assert_balances(&account, 0.0, 100.0, 100.0);
        assert_eq!(account.disputes.get(&1), Some(&DisputeState::Disputed));
    }

    #[test]
    fn test_dispute_unknown_tx_ignored() {
        let mut account = ClientAccount::new(1);
        account.settle_transaction(make_deposit(1, 100.0));
        account.adjudicate_claim(make_dispute(999)); // Unknown tx

        assert_balances(&account, 100.0, 0.0, 100.0);
        assert!(!account.disputes.contains_key(&999));
    }

    #[test]
    fn test_dispute_withdrawal_rejected() {
        let mut account = ClientAccount::new(1);
        account.settle_transaction(make_deposit(1, 100.0));
        account.settle_transaction(make_withdrawal(2, 30.0));
        account.adjudicate_claim(make_dispute(2)); // Try to dispute withdrawal

        assert_balances(&account, 70.0, 0.0, 70.0);
        assert!(!account.disputes.contains_key(&2));
    }

    #[test]
    fn test_dispute_already_disputed_rejected() {
        let mut account = ClientAccount::new(1);
        account.settle_transaction(make_deposit(1, 100.0));
        account.adjudicate_claim(make_dispute(1));
        account.adjudicate_claim(make_dispute(1)); // Duplicate

        // Should NOT double-hold
        assert_balances(&account, 0.0, 100.0, 100.0);
    }

    #[test]
    fn test_dispute_on_locked_account_rejected() {
        let mut account = ClientAccount::new(1);
        account.settle_transaction(make_deposit(1, 100.0));
        account.locked = true;
        account.adjudicate_claim(make_dispute(1));

        // Dispute should be rejected
        assert_balances(&account, 100.0, 0.0, 100.0);
        assert!(!account.disputes.contains_key(&1));
    }

    #[test]
    fn test_resolve_releases_funds() {
        let mut account = ClientAccount::new(1);
        account.settle_transaction(make_deposit(1, 100.0));
        account.adjudicate_claim(make_dispute(1));
        account.adjudicate_claim(make_resolve(1));

        assert_balances(&account, 100.0, 0.0, 100.0);
        assert_eq!(account.disputes.get(&1), Some(&DisputeState::Resolved));
        assert!(!account.locked);
    }

    #[test]
    fn test_resolve_unknown_tx_ignored() {
        let mut account = ClientAccount::new(1);
        account.settle_transaction(make_deposit(1, 100.0));
        account.adjudicate_claim(make_resolve(999));

        assert_balances(&account, 100.0, 0.0, 100.0);
    }

    #[test]
    fn test_resolve_not_disputed_ignored() {
        let mut account = ClientAccount::new(1);
        account.settle_transaction(make_deposit(1, 100.0));
        account.adjudicate_claim(make_resolve(1)); // Not disputed

        assert_balances(&account, 100.0, 0.0, 100.0);
    }

    #[test]
    fn test_resolve_already_resolved_ignored() {
        let mut account = ClientAccount::new(1);
        account.settle_transaction(make_deposit(1, 100.0));
        account.adjudicate_claim(make_dispute(1));
        account.adjudicate_claim(make_resolve(1));
        account.adjudicate_claim(make_resolve(1)); // Double resolve

        // Should NOT double-release
        assert_balances(&account, 100.0, 0.0, 100.0);
    }

    #[test]
    fn test_resolve_on_locked_account_allowed() {
        // Pre-freeze disputes can still be resolved
        let mut account = ClientAccount::new(1);
        account.settle_transaction(make_deposit(1, 100.0));
        account.adjudicate_claim(make_dispute(1));
        account.locked = true;
        account.adjudicate_claim(make_resolve(1));

        assert_balances(&account, 100.0, 0.0, 100.0);
        assert_eq!(account.disputes.get(&1), Some(&DisputeState::Resolved));
    }

    #[test]
    fn test_chargeback_removes_funds_and_locks() {
        let mut account = ClientAccount::new(1);
        account.settle_transaction(make_deposit(1, 100.0));
        account.adjudicate_claim(make_dispute(1));
        account.adjudicate_claim(make_chargeback(1));

        assert_balances(&account, 0.0, 0.0, 0.0);
        assert_eq!(account.disputes.get(&1), Some(&DisputeState::ChargedBack));
        assert!(account.locked);
    }

    #[test]
    fn test_chargeback_unknown_tx_ignored() {
        let mut account = ClientAccount::new(1);
        account.settle_transaction(make_deposit(1, 100.0));
        account.adjudicate_claim(make_chargeback(999));

        assert_balances(&account, 100.0, 0.0, 100.0);
        assert!(!account.locked);
    }

    #[test]
    fn test_chargeback_not_disputed_ignored() {
        let mut account = ClientAccount::new(1);
        account.settle_transaction(make_deposit(1, 100.0));
        account.adjudicate_claim(make_chargeback(1)); // Not disputed

        assert_balances(&account, 100.0, 0.0, 100.0);
        assert!(!account.locked);
    }

    #[test]
    fn test_chargeback_already_resolved_ignored() {
        let mut account = ClientAccount::new(1);
        account.settle_transaction(make_deposit(1, 100.0));
        account.adjudicate_claim(make_dispute(1));
        account.adjudicate_claim(make_resolve(1));
        account.adjudicate_claim(make_chargeback(1)); // Already resolved

        assert_balances(&account, 100.0, 0.0, 100.0);
        assert!(!account.locked);
    }

    #[test]
    fn test_chargeback_already_chargedback_ignored() {
        let mut account = ClientAccount::new(1);
        account.settle_transaction(make_deposit(1, 100.0));
        account.settle_transaction(make_deposit(2, 50.0));
        account.adjudicate_claim(make_dispute(1));
        account.adjudicate_claim(make_chargeback(1));
        account.adjudicate_claim(make_chargeback(1)); // Double chargeback

        // Should NOT double-remove
        assert_balances(&account, 50.0, 0.0, 50.0);
    }

    #[test]
    fn test_chargeback_on_locked_account_allowed() {
        // Pre-freeze disputes can still be charged back
        let mut account = ClientAccount::new(1);
        account.settle_transaction(make_deposit(1, 100.0));
        account.settle_transaction(make_deposit(2, 50.0));
        account.adjudicate_claim(make_dispute(1));
        account.adjudicate_claim(make_dispute(2));
        account.adjudicate_claim(make_chargeback(1)); // Locks account
        account.adjudicate_claim(make_chargeback(2)); // Pre-freeze dispute

        assert_balances(&account, 0.0, 0.0, 0.0);
    }

    #[test]
    fn test_dispute_after_partial_spend_goes_negative() {
        let mut account = ClientAccount::new(1);
        account.settle_transaction(make_deposit(1, 100.0));
        account.settle_transaction(make_withdrawal(2, 70.0));
        account.adjudicate_claim(make_dispute(1));

        // Available goes negative
        assert_balances(&account, -70.0, 100.0, 30.0);
    }

    #[test]
    fn test_chargeback_after_partial_spend_negative_total() {
        let mut account = ClientAccount::new(1);
        account.settle_transaction(make_deposit(1, 100.0));
        account.settle_transaction(make_withdrawal(2, 70.0));
        account.adjudicate_claim(make_dispute(1));
        account.adjudicate_claim(make_chargeback(1));

        // Client owes money
        assert_balances(&account, -70.0, 0.0, -70.0);
        assert!(account.locked);
    }

    // ========== Edge Case Tests ==========

    #[test]
    fn test_negative_amount_deposit_rejected() {
        let mut account = ClientAccount::new(1);
        let tx = Transaction {
            transaction_type: TransactionType::Deposit,
            amount: Some(Decimal::from_f64(-100.0)),
            tx: 1,
            client: 1,
        };
        account.settle_transaction(tx);

        // Balance unchanged, not recorded in ledger
        assert_balances(&account, 0.0, 0.0, 0.0);
        assert!(!account.ledger.contains_key(&1));
    }

    #[test]
    fn test_negative_amount_withdrawal_rejected() {
        let mut account = ClientAccount::new(1);
        account.settle_transaction(make_deposit(1, 100.0));
        let tx = Transaction {
            transaction_type: TransactionType::Withdrawal,
            amount: Some(Decimal::from_f64(-50.0)),
            tx: 2,
            client: 1,
        };
        account.settle_transaction(tx);

        // Balance unchanged from deposit
        assert_balances(&account, 100.0, 0.0, 100.0);
        assert!(!account.ledger.contains_key(&2));
    }

    #[test]
    fn test_zero_amount_deposit() {
        let mut account = ClientAccount::new(1);
        let tx = Transaction {
            transaction_type: TransactionType::Deposit,
            amount: Some(Decimal::from_f64(0.0)),
            tx: 1,
            client: 1,
        };
        account.settle_transaction(tx);

        // Zero deposit is valid, recorded in ledger
        assert_balances(&account, 0.0, 0.0, 0.0);
        assert!(account.ledger.contains_key(&1));
    }

    #[test]
    fn test_zero_amount_deposit_can_be_disputed() {
        let mut account = ClientAccount::new(1);
        let tx = Transaction {
            transaction_type: TransactionType::Deposit,
            amount: Some(Decimal::from_f64(0.0)),
            tx: 1,
            client: 1,
        };
        account.settle_transaction(tx);
        account.adjudicate_claim(make_dispute(1));

        // Zero held
        assert_balances(&account, 0.0, 0.0, 0.0);
        assert!(account.disputes.get(&1).is_some());
    }

    #[test]
    fn test_boundary_tx_id_zero() {
        let mut account = ClientAccount::new(1);
        let tx = Transaction {
            transaction_type: TransactionType::Deposit,
            amount: Some(Decimal::from_f64(100.0)),
            tx: 0, // Minimum tx ID
            client: 1,
        };
        account.settle_transaction(tx);

        assert_balances(&account, 100.0, 0.0, 100.0);
        assert!(account.ledger.contains_key(&0));
    }

    #[test]
    fn test_boundary_tx_id_max() {
        let mut account = ClientAccount::new(1);
        let tx = Transaction {
            transaction_type: TransactionType::Deposit,
            amount: Some(Decimal::from_f64(100.0)),
            tx: u32::MAX, // Maximum tx ID
            client: 1,
        };
        account.settle_transaction(tx);

        assert_balances(&account, 100.0, 0.0, 100.0);
        assert!(account.ledger.contains_key(&u32::MAX));
    }

    #[test]
    fn test_boundary_client_id_zero() {
        let account = ClientAccount::new(0); // Minimum client ID
        assert_eq!(account.client_id, 0);
    }

    #[test]
    fn test_boundary_client_id_max() {
        let account = ClientAccount::new(u16::MAX); // Maximum client ID
        assert_eq!(account.client_id, u16::MAX);
    }
}
