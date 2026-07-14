//! Pure domain entity errors
//!
//! This module defines domain entity-specific errors
//! without external framework dependencies, following
//! Clean Architecture principles.
//!
//! Errors manually implement `std::error::Error` and `std::fmt::Display`
//! to keep the domain free of external dependencies.

use std::error::Error;
use std::fmt::{Display, Formatter, Result as FmtResult};

// ============================================================================
// FILE ERRORS
// ============================================================================

/// Errors that can occur during File entity operations
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileError {
    /// Occurs when the file name contains invalid characters or is empty
    InvalidFileName(String),
    /// Occurs when validation of any entity attribute fails
    ValidationError(String),
}

impl Display for FileError {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        match self {
            FileError::InvalidFileName(name) => write!(f, "Invalid file name: {}", name),
            FileError::ValidationError(msg) => write!(f, "Validation error: {}", msg),
        }
    }
}

impl Error for FileError {}

/// Type alias for File entity operation results
pub type FileResult<T> = Result<T, FileError>;

// ============================================================================
// FOLDER ERRORS
// ============================================================================

/// Errors that can occur during Folder entity operations
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FolderError {
    /// Occurs when the folder name contains invalid characters or is empty
    InvalidFolderName(String),
    /// Occurs when validation of any entity attribute fails
    ValidationError(String),
}

impl Display for FolderError {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        match self {
            FolderError::InvalidFolderName(name) => write!(f, "Invalid folder name: {}", name),
            FolderError::ValidationError(msg) => write!(f, "Validation error: {}", msg),
        }
    }
}

impl Error for FolderError {}

/// Type alias for Folder entity operation results
pub type FolderResult<T> = Result<T, FolderError>;

// ============================================================================
// USER ERRORS
// ============================================================================

/// Errors that can occur during User entity operations
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserError {
    /// Invalid username
    InvalidUsername(String),
    /// Invalid password
    InvalidPassword(String),
    /// General validation error
    ValidationError(String),
    /// Authentication error
    AuthenticationError(String),
    /// Upgrade path: the user is already internal — cannot re-upgrade.
    /// Surfaced by the service as `error_type = "AlreadyInternal"`.
    AlreadyInternal,
}

impl Display for UserError {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        match self {
            UserError::InvalidUsername(msg) => write!(f, "Invalid username: {}", msg),
            UserError::InvalidPassword(msg) => write!(f, "Invalid password: {}", msg),
            UserError::ValidationError(msg) => write!(f, "Validation error: {}", msg),
            UserError::AuthenticationError(msg) => write!(f, "Authentication error: {}", msg),
            UserError::AlreadyInternal => write!(f, "User is already an internal account"),
        }
    }
}

impl Error for UserError {}

/// Type alias for User entity operation results
pub type UserResult<T> = Result<T, UserError>;

// ============================================================================
// SHARE ERRORS
// ============================================================================

/// Errors that can occur during Share entity operations
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShareError {
    /// Invalid share token
    InvalidToken(String),
    /// Invalid expiration date
    InvalidExpiration(String),
    /// General validation error
    ValidationError(String),
}

impl Display for ShareError {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        match self {
            ShareError::InvalidToken(msg) => write!(f, "Invalid token: {}", msg),
            ShareError::InvalidExpiration(msg) => write!(f, "Invalid expiration date: {}", msg),
            ShareError::ValidationError(msg) => write!(f, "Validation error: {}", msg),
        }
    }
}

impl Error for ShareError {}

/// Type alias for Share entity operation results
pub type ShareResult<T> = Result<T, ShareError>;

// ============================================================================
// CALENDAR ERRORS
// ============================================================================

/// Errors that can occur during Calendar entity operations
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CalendarError {
    /// Invalid calendar name
    InvalidName(String),
    /// Invalid color code
    InvalidColor(String),
    /// Invalid owner ID
    InvalidOwnerId(String),
}

impl Display for CalendarError {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        match self {
            CalendarError::InvalidName(msg) => write!(f, "Invalid calendar name: {}", msg),
            CalendarError::InvalidColor(msg) => write!(f, "Invalid color code: {}", msg),
            CalendarError::InvalidOwnerId(msg) => write!(f, "Invalid owner ID: {}", msg),
        }
    }
}

impl Error for CalendarError {}

/// Type alias for Calendar entity operation results
pub type CalendarResult<T> = Result<T, CalendarError>;

// ============================================================================
// CALENDAR EVENT ERRORS
// ============================================================================

/// Errors that can occur during CalendarEvent entity operations
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CalendarEventError {
    /// Invalid event summary/title
    InvalidSummary(String),
    /// Invalid event dates
    InvalidDates(String),
    /// Invalid recurrence rule
    InvalidRecurrence(String),
    /// Invalid iCalendar data
    InvalidICalData(String),
}

impl Display for CalendarEventError {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        match self {
            CalendarEventError::InvalidSummary(msg) => write!(f, "Invalid event summary: {}", msg),
            CalendarEventError::InvalidDates(msg) => write!(f, "Invalid event dates: {}", msg),
            CalendarEventError::InvalidRecurrence(msg) => {
                write!(f, "Invalid recurrence rule: {}", msg)
            }
            CalendarEventError::InvalidICalData(msg) => {
                write!(f, "Invalid iCalendar data: {}", msg)
            }
        }
    }
}

impl Error for CalendarEventError {}

/// Type alias for CalendarEvent entity operation results
pub type CalendarEventResult<T> = Result<T, CalendarEventError>;

// ============================================================================
// SUBJECT GROUP ERRORS
// ============================================================================

/// Errors that can occur during SubjectGroup entity operations.
///
/// Subject groups are ReBAC authorization principals: root-owned, named with
/// RFC 5321 local-part shape (so they may later be addressed as email locals),
/// and able to nest (a group can contain other groups, with cycle detection
/// at write time at the application layer).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubjectGroupError {
    /// Name doesn't match the RFC 5321 local-part shape (alnum start, then
    /// alnum/dot/dash/underscore, max 64 chars).
    InvalidName(String),
    /// Cycle detected — the proposed membership would create a loop in the
    /// group-of-groups graph.
    CycleDetected(String),
    /// Adding this member would exceed the maximum nesting depth.
    DepthExceeded(String),
    /// Attempt to mutate (delete, rename, or change membership of) an
    /// immutable virtual group such as `Internal`.
    VirtualImmutable(String),
    /// General validation error.
    ValidationError(String),
}

impl Display for SubjectGroupError {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        match self {
            SubjectGroupError::InvalidName(msg) => write!(f, "Invalid group name: {}", msg),
            SubjectGroupError::CycleDetected(msg) => write!(f, "Cycle detected: {}", msg),
            SubjectGroupError::DepthExceeded(msg) => write!(f, "Group depth exceeded: {}", msg),
            SubjectGroupError::VirtualImmutable(msg) => {
                write!(f, "Virtual group is immutable: {}", msg)
            }
            SubjectGroupError::ValidationError(msg) => write!(f, "Validation error: {}", msg),
        }
    }
}

impl Error for SubjectGroupError {}

/// Type alias for SubjectGroup entity operation results.
pub type SubjectGroupResult<T> = Result<T, SubjectGroupError>;

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_error_display() {
        let err = FileError::InvalidFileName("test.txt".to_string());
        assert_eq!(err.to_string(), "Invalid file name: test.txt");

        let err = FileError::ValidationError("size too large".to_string());
        assert_eq!(err.to_string(), "Validation error: size too large");
    }

    #[test]
    fn test_folder_error_display() {
        let err = FolderError::InvalidFolderName("my/folder".to_string());
        assert_eq!(err.to_string(), "Invalid folder name: my/folder");
    }

    #[test]
    fn test_user_error_display() {
        let err = UserError::InvalidUsername("".to_string());
        assert_eq!(err.to_string(), "Invalid username: ");

        let err = UserError::AuthenticationError("invalid credentials".to_string());
        assert_eq!(err.to_string(), "Authentication error: invalid credentials");
    }

    #[test]
    fn test_share_error_display() {
        let err = ShareError::InvalidToken("abc123".to_string());
        assert_eq!(err.to_string(), "Invalid token: abc123");
    }

    #[test]
    fn test_calendar_error_display() {
        let err = CalendarError::InvalidColor("not-a-color".to_string());
        assert_eq!(err.to_string(), "Invalid color code: not-a-color");
    }

    #[test]
    fn test_calendar_event_error_display() {
        let err = CalendarEventError::InvalidDates("end before start".to_string());
        assert_eq!(err.to_string(), "Invalid event dates: end before start");
    }

    #[test]
    fn test_errors_implement_error_trait() {
        fn assert_error<E: Error>() {}

        assert_error::<FileError>();
        assert_error::<FolderError>();
        assert_error::<UserError>();
        assert_error::<ShareError>();
        assert_error::<CalendarError>();
        assert_error::<CalendarEventError>();
        assert_error::<SubjectGroupError>();
    }

    #[test]
    fn test_subject_group_error_display() {
        let err = SubjectGroupError::InvalidName("Engineering Team".to_string());
        assert_eq!(err.to_string(), "Invalid group name: Engineering Team");

        let err = SubjectGroupError::CycleDetected("qa → engineering → qa".to_string());
        assert_eq!(err.to_string(), "Cycle detected: qa → engineering → qa");

        let err = SubjectGroupError::DepthExceeded("would reach depth 9".to_string());
        assert_eq!(err.to_string(), "Group depth exceeded: would reach depth 9");
    }
}
