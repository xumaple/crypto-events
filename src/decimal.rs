//! Fixed-point decimal implementation for precise financial calculations.
//!
//! Avoids floating-point precision issues by storing values as integers
//! with 4 decimal places of precision (i.e., value × 10,000).

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::ops::{Add, AddAssign, Sub, SubAssign};

/// Fixed-point decimal with 4 decimal places.
///
/// Stores value * 10000 internally (e.g., 1.5 is stored as 15000).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Decimal(pub i64);

impl Serialize for Decimal {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Serialize as f64 for CSV output
        let value = self.0 as f64 / 10000.0;
        serializer.serialize_f64(value)
    }
}

impl<'de> Deserialize<'de> for Decimal {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = f64::deserialize(deserializer)?;
        Ok(Decimal::from_f64(value))
    }
}

impl fmt::Display for Decimal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let abs = self.0.abs();
        let whole = abs / 10000;
        let frac = abs % 10000;

        if self.0 < 0 {
            write!(f, "-")?;
        }

        if frac == 0 {
            write!(f, "{}", whole)
        } else {
            // Remove trailing zeros from fraction
            let mut frac_str = format!("{:04}", frac);
            frac_str = frac_str.trim_end_matches('0').to_string();
            write!(f, "{}.{}", whole, frac_str)
        }
    }
}

impl Decimal {
    /// Create from raw internal representation (value in ten-thousandths).
    ///
    /// E.g., `Decimal::new(15000)` represents 1.5
    pub fn new(value: i64) -> Self {
        Self(value)
    }

    /// Create from a float.
    /// E.g., from_f64(1.5) => Decimal(15000)
    pub fn from_f64(value: f64) -> Self {
        Self((value * 10000.0).round() as i64)
    }
}

impl AddAssign for Decimal {
    fn add_assign(&mut self, other: Self) {
        self.0 += other.0;
    }
}

impl SubAssign for Decimal {
    fn sub_assign(&mut self, other: Self) {
        self.0 -= other.0;
    }
}

impl Add for Decimal {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self(self.0 + other.0)
    }
}

impl Sub for Decimal {
    type Output = Self;

    fn sub(self, other: Self) -> Self {
        Self(self.0 - other.0)
    }
}

#[cfg(test)]
mod display_tests {
    use super::*;

    #[test]
    fn test_display_whole_numbers() {
        assert_eq!(Decimal(0).to_string(), "0");
        assert_eq!(Decimal(10000).to_string(), "1");
        assert_eq!(Decimal(20000).to_string(), "2");
        assert_eq!(Decimal(1000000).to_string(), "100");
    }

    #[test]
    fn test_display_with_decimals() {
        assert_eq!(Decimal(15000).to_string(), "1.5");
        assert_eq!(Decimal(12500).to_string(), "1.25");
        assert_eq!(Decimal(12340).to_string(), "1.234");
        assert_eq!(Decimal(12345).to_string(), "1.2345");
    }

    #[test]
    fn test_display_fractional_only() {
        assert_eq!(Decimal(5000).to_string(), "0.5");
        assert_eq!(Decimal(1).to_string(), "0.0001");
        assert_eq!(Decimal(10).to_string(), "0.001");
        assert_eq!(Decimal(100).to_string(), "0.01");
        assert_eq!(Decimal(1000).to_string(), "0.1");
    }

    #[test]
    fn test_display_negative_numbers() {
        assert_eq!(Decimal(-10000).to_string(), "-1");
        assert_eq!(Decimal(-15000).to_string(), "-1.5");
        assert_eq!(Decimal(-12345).to_string(), "-1.2345");
        assert_eq!(Decimal(-5000).to_string(), "-0.5");
        assert_eq!(Decimal(-1).to_string(), "-0.0001");
    }

    #[test]
    fn test_display_large_numbers() {
        assert_eq!(Decimal(99999999990000).to_string(), "9999999999");
        assert_eq!(Decimal(123456789012345).to_string(), "12345678901.2345");
    }
}

#[cfg(test)]
mod deserialize_tests {
    use super::*;

    fn deserialize(s: &str) -> Decimal {
        serde_json::from_str(s).unwrap()
    }

    #[test]
    fn test_deserialize_whole_numbers() {
        assert_eq!(deserialize("0"), Decimal(0));
        assert_eq!(deserialize("1"), Decimal(10000));
        assert_eq!(deserialize("100"), Decimal(1000000));
    }

    #[test]
    fn test_deserialize_with_decimals() {
        assert_eq!(deserialize("1.5"), Decimal(15000));
        assert_eq!(deserialize("1.25"), Decimal(12500));
        assert_eq!(deserialize("1.2345"), Decimal(12345));
    }

    #[test]
    fn test_deserialize_fractional_only() {
        assert_eq!(deserialize("0.5"), Decimal(5000));
        assert_eq!(deserialize("0.0001"), Decimal(1));
        assert_eq!(deserialize("0.001"), Decimal(10));
        assert_eq!(deserialize("0.01"), Decimal(100));
        assert_eq!(deserialize("0.1"), Decimal(1000));
    }

    #[test]
    fn test_deserialize_negative_numbers() {
        assert_eq!(deserialize("-1"), Decimal(-10000));
        assert_eq!(deserialize("-1.5"), Decimal(-15000));
        assert_eq!(deserialize("-0.5"), Decimal(-5000));
    }

    #[test]
    fn test_deserialize_roundtrip() {
        let values = vec![
            Decimal(0),
            Decimal(10000),
            Decimal(15000),
            Decimal(12345),
            Decimal(-15000),
            Decimal(1),
            Decimal(-1),
        ];
        for original in values {
            let serialized = serde_json::to_string(&original).unwrap();
            let deserialized: Decimal = serde_json::from_str(&serialized).unwrap();
            assert_eq!(
                original, deserialized,
                "roundtrip failed for {:?}",
                original
            );
        }
    }

    #[test]
    fn test_deserialize_five_plus_decimal_places_rounded() {
        // 5+ decimal places are rounded to 4 (via f64 * 10000 then round)
        assert_eq!(deserialize("1.23456"), Decimal(12346)); // rounds up
        assert_eq!(deserialize("1.23454"), Decimal(12345)); // rounds down
        assert_eq!(deserialize("1.234567890"), Decimal(12346));
        assert_eq!(deserialize("0.00001"), Decimal(0)); // too small, rounds to 0
        assert_eq!(deserialize("0.00005"), Decimal(1)); // rounds up to 0.0001
        assert_eq!(deserialize("0.00004"), Decimal(0)); // rounds down to 0
    }
}

#[cfg(test)]
mod arithmetic_tests {
    use super::*;

    #[test]
    fn test_add() {
        assert_eq!(Decimal(10000) + Decimal(5000), Decimal(15000));
        assert_eq!(Decimal(0) + Decimal(10000), Decimal(10000));
        assert_eq!(Decimal(-5000) + Decimal(10000), Decimal(5000));
        assert_eq!(Decimal(-5000) + Decimal(-5000), Decimal(-10000));
    }

    #[test]
    fn test_sub() {
        assert_eq!(Decimal(10000) - Decimal(5000), Decimal(5000));
        assert_eq!(Decimal(5000) - Decimal(10000), Decimal(-5000));
        assert_eq!(Decimal(0) - Decimal(10000), Decimal(-10000));
        assert_eq!(Decimal(-5000) - Decimal(-5000), Decimal(0));
    }

    #[test]
    fn test_add_assign() {
        let mut d = Decimal(10000);
        d += Decimal(5000);
        assert_eq!(d, Decimal(15000));

        d += Decimal(-20000);
        assert_eq!(d, Decimal(-5000));
    }

    #[test]
    fn test_sub_assign() {
        let mut d = Decimal(10000);
        d -= Decimal(3000);
        assert_eq!(d, Decimal(7000));

        d -= Decimal(10000);
        assert_eq!(d, Decimal(-3000));
    }
}
