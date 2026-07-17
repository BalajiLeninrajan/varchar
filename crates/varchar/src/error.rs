use thiserror::Error;

use crate::Resource;

/// A half-open byte range in the original UTF-8 SQL input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    /// Inclusive start byte offset.
    pub start: usize,
    /// Exclusive end byte offset.
    pub end: usize,
}

impl Span {
    pub(crate) const fn new(start: usize, end: usize) -> Self {
        assert!(start <= end, "a SQL span must be ordered");
        Self { start, end }
    }
}

/// An error produced by the platform-neutral database core.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// The SQL input could not be parsed.
    #[error("SQL parse error at bytes {span_start}..{span_end}: {message}")]
    Parse {
        /// Human-readable diagnostic detail.
        message: String,
        /// Inclusive start byte offset in the SQL input.
        span_start: usize,
        /// Exclusive end byte offset in the SQL input.
        span_end: usize,
    },

    /// The SQL input uses recognized but unsupported syntax.
    #[error("unsupported SQL feature at bytes {span_start}..{span_end}: {feature}")]
    Unsupported {
        /// Human-readable description of the unsupported syntax.
        feature: String,
        /// Inclusive start byte offset in the SQL input.
        span_start: usize,
        /// Exclusive end byte offset in the SQL input.
        span_end: usize,
    },

    /// A table, column, or schema declaration is invalid.
    #[error("schema error: {0}")]
    Schema(String),

    /// A value or expression has an incompatible type.
    #[error("type error: {0}")]
    Type(String),

    /// A primary-key, foreign-key, or related relational constraint failed.
    #[error("constraint violation: {0}")]
    Constraint(String),

    /// The encoded database string is invalid.
    #[error("corrupt database at byte {offset}: {message}")]
    CorruptStorage {
        /// Byte offset in the encoded database string.
        offset: usize,
        /// Human-readable diagnostic detail.
        message: String,
    },

    /// A generated regular expression could not be compiled.
    #[error("generated regex could not be compiled: {0}")]
    RegexCompile(String),

    /// A compiled regular expression failed while executing.
    #[error("regex execution failed: {0}")]
    RegexRuntime(String),

    /// A configured resource policy was exceeded.
    #[error("resource limit exceeded for {resource} (limit: {limit})")]
    ResourceLimit {
        /// The configured resource that was exceeded.
        resource: Resource,
        /// The configured maximum.
        limit: usize,
    },

    /// An explicit memory reservation failed.
    #[error("allocation failed while {operation}")]
    Allocation {
        /// The operation whose reservation failed.
        operation: &'static str,
    },

    /// Arithmetic or count overflow prevented internal growth.
    #[error("capacity exceeded while {operation}")]
    Capacity {
        /// The operation whose capacity was exceeded.
        operation: &'static str,
    },
}

impl Error {
    pub(crate) fn parse(message: impl Into<String>, span: Span) -> Self {
        Self::Parse {
            message: message.into(),
            span_start: span.start,
            span_end: span.end,
        }
    }

    pub(crate) fn unsupported(feature: impl Into<String>, span: Span) -> Self {
        Self::Unsupported {
            feature: feature.into(),
            span_start: span.start,
            span_end: span.end,
        }
    }
}

/// A result returned by the varchar core.
pub type Result<T> = std::result::Result<T, Error>;
