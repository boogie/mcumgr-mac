//! SMP management group / command identifiers and return codes.

/// Management groups defined by the SMP protocol.
///
/// Only OS and Image are exercised by this CLI, but the full set is listed
/// for clarity and future use.
pub mod group {
    pub const OS: u16 = 0;
    pub const IMAGE: u16 = 1;
    pub const STAT: u16 = 2;
    pub const CONFIG: u16 = 3;
    pub const LOG: u16 = 4;
    pub const CRASH: u16 = 5;
    pub const SPLIT: u16 = 6;
    pub const RUN: u16 = 7;
    pub const FS: u16 = 8;
    pub const SHELL: u16 = 9;
}

/// Command identifiers within the OS management group.
pub mod os {
    pub const ECHO: u8 = 0;
    pub const CONS_ECHO_CTRL: u8 = 1;
    pub const TASKSTAT: u8 = 2;
    pub const MPSTAT: u8 = 3;
    pub const DATETIME_STR: u8 = 4;
    pub const RESET: u8 = 5;
}

/// Command identifiers within the Image management group.
pub mod image {
    pub const STATE: u8 = 0;
    pub const UPLOAD: u8 = 1;
    pub const FILE: u8 = 2;
    pub const CORELIST: u8 = 3;
    pub const CORELOAD: u8 = 4;
    pub const ERASE: u8 = 5;
}

/// A non-zero SMP management return code (`rc`) reported by a device.
///
/// `rc == 0` (or an absent `rc`) means success and never produces this error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MgmtError {
    /// The raw return code reported by the device.
    pub code: u16,
}

impl MgmtError {
    /// Construct an error from a raw return code.
    pub fn new(code: u16) -> Self {
        Self { code }
    }

    /// A human-readable description of this return code.
    ///
    /// Codes follow the standard `MGMT_ERR_*` table; unknown codes fall back
    /// to a generic message.
    pub fn description(&self) -> &'static str {
        match self.code {
            1 => "unknown error",
            2 => "out of memory",
            3 => "invalid value",
            4 => "operation timed out",
            5 => "no such entry",
            6 => "bad state for operation",
            7 => "response too large",
            8 => "operation not supported",
            9 => "data corruption detected",
            10 => "resource busy",
            11 => "access denied",
            12 => "unsupported format (too old)",
            13 => "unsupported format (too new)",
            _ => "unknown management error",
        }
    }
}

impl std::fmt::Display for MgmtError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (rc={})", self.description(), self.code)
    }
}

impl std::error::Error for MgmtError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describes_known_return_codes() {
        assert_eq!(MgmtError::new(1).description(), "unknown error");
        assert_eq!(MgmtError::new(2).description(), "out of memory");
        assert_eq!(MgmtError::new(3).description(), "invalid value");
        assert_eq!(MgmtError::new(8).description(), "operation not supported");
        assert_eq!(MgmtError::new(11).description(), "access denied");
    }

    #[test]
    fn describes_unknown_return_codes_generically() {
        assert_eq!(
            MgmtError::new(9999).description(),
            "unknown management error"
        );
    }

    #[test]
    fn display_includes_code() {
        assert_eq!(MgmtError::new(3).to_string(), "invalid value (rc=3)");
    }
}
