//! Super simple tracing macros which emulate the `tracing` crate.
//!
//! Logs are printed to stderr with level prefixes.

/// Logs an info message.
#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => {
        eprintln!("[INFO] {}", format!($($arg)*))
    };
}

/// Logs an error message.
#[macro_export]
macro_rules! error {
    ($($arg:tt)*) => {
        eprintln!("[ERROR] {}", format!($($arg)*))
    };
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_info_macro() {
        info!("abc{}", "def"); // prints [INFO] abcdef
    }

    #[test]
    fn test_error_macro() {
        error!("ghi{}", "jkl"); // prints [ERROR] ghijkl
    }

    #[test]
    fn test_with_variables() {
        let x = 42;
        let name = "test";
        info!("value: {}, name: {}", x, name); // prints [INFO] value: 42, name: test
    }
}
