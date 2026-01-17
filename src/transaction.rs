//! Transaction types and structures for the payments engine.
//!
//! Defines the core [`Transaction`] struct that represents a single operation
//! (deposit, withdrawal, dispute, resolve, or chargeback) read from CSV input.

use serde::Deserialize;

use crate::{ClientId, TransactionId, decimal::Decimal};

/// Transaction types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransactionType {
    Deposit,
    Withdrawal,
    Dispute,
    Resolve,
    Chargeback,
}

/// Transaction record.
#[derive(Debug, Deserialize, Clone, Copy)]
pub struct Transaction {
    #[serde(rename = "type")]
    pub transaction_type: TransactionType,
    pub amount: Option<Decimal>,
    pub tx: TransactionId,
    pub client: ClientId,
}

impl Transaction {
    /// Returns true if the transaction type is dispute, resolve, or chargeback.
    pub fn is_dispute_related(&self) -> bool {
        matches!(
            self.transaction_type,
            TransactionType::Dispute | TransactionType::Resolve | TransactionType::Chargeback
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_dispute_related() {
        let make_tx = |t: TransactionType| Transaction {
            transaction_type: t,
            amount: None,
            tx: 1,
            client: 1,
        };

        assert!(!make_tx(TransactionType::Deposit).is_dispute_related());
        assert!(!make_tx(TransactionType::Withdrawal).is_dispute_related());
        assert!(make_tx(TransactionType::Dispute).is_dispute_related());
        assert!(make_tx(TransactionType::Resolve).is_dispute_related());
        assert!(make_tx(TransactionType::Chargeback).is_dispute_related());
    }
}
