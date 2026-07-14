use std::fmt;
use std::ops::Range;

use crate::Resource;

/// A half-open byte range in the original UTF-8 SQL input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    start: usize,
    end: usize,
}

impl Span {
    pub(crate) const fn new(start: usize, end: usize) -> Self {
        assert!(start <= end, "a SQL span must be ordered");
        Self { start, end }
    }

    /// Return the inclusive start byte offset.
    #[must_use]
    pub const fn start(self) -> usize {
        self.start
    }

    /// Return the exclusive end byte offset.
    #[must_use]
    pub const fn end(self) -> usize {
        self.end
    }

    /// Return the length of this byte range.
    #[must_use]
    pub const fn len(self) -> usize {
        self.end - self.start
    }

    /// Return whether this byte range is empty.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.start == self.end
    }

    /// Return this span as a standard half-open range.
    #[must_use]
    pub const fn range(self) -> Range<usize> {
        self.start..self.end
    }
}

/// A stable category for a [`crate::Error`].
///
/// Use [`ErrorCode::as_str`] for telemetry and other persisted identifiers.
/// Human-readable error messages are intentionally not a stable interface.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorCode {
    /// The SQL input could not be parsed.
    SqlParse,
    /// The SQL input uses syntax that varchar recognizes but does not support.
    UnsupportedSql,
    /// A table, column, or schema declaration is invalid.
    Schema,
    /// A value or expression has an incompatible type.
    Type,
    /// A primary-key, foreign-key, or related relational constraint failed.
    Constraint,
    /// The encoded database string is invalid.
    CorruptStorage,
    /// A generated regular expression could not be compiled.
    RegexCompile,
    /// A compiled regular expression failed while executing.
    RegexRuntime,
    /// A configured resource policy was exceeded.
    ResourceLimit,
    /// An explicit memory reservation failed.
    Allocation,
    /// Detected arithmetic or count overflow prevented internal growth.
    Capacity,
}

impl ErrorCode {
    /// Return a stable machine-readable name for this error category.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SqlParse => "sql_parse",
            Self::UnsupportedSql => "unsupported_sql",
            Self::Schema => "schema",
            Self::Type => "type",
            Self::Constraint => "constraint",
            Self::CorruptStorage => "corrupt_storage",
            Self::RegexCompile => "regex_compile",
            Self::RegexRuntime => "regex_runtime",
            Self::ResourceLimit => "resource_limit",
            Self::Allocation => "allocation",
            Self::Capacity => "capacity",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::SqlParse => "SQL parse error",
            Self::UnsupportedSql => "unsupported SQL",
            Self::Schema => "schema error",
            Self::Type => "type error",
            Self::Constraint => "constraint violation",
            Self::CorruptStorage => "corrupt storage",
            Self::RegexCompile => "regex compilation error",
            Self::RegexRuntime => "regex runtime error",
            Self::ResourceLimit => "resource limit",
            Self::Allocation => "allocation failure",
            Self::Capacity => "capacity exceeded",
        })
    }
}

/// An error produced by the platform-neutral database core.
///
/// The concrete representation is private so callers can classify failures
/// through [`Error::code`] without depending on diagnostic wording. SQL
/// syntax diagnostics may include a [`Span`], corrupt-storage diagnostics may
/// include an encoded-blob byte offset, and configured limit failures include
/// both a [`Resource`] and its limit. The `Display` and `Debug` formats are
/// human-facing diagnostics and are not stable interfaces.
pub struct Error {
    kind: ErrorKind,
}

#[derive(Debug)]
enum ErrorKind {
    Parse { message: String, span: Span },
    Unsupported { feature: String, span: Span },
    Schema(String),
    Type(String),
    Constraint(String),
    CorruptStorage { offset: usize, message: String },
    RegexCompile(String),
    RegexRuntime(String),
    Allocation { operation: &'static str },
    Capacity { operation: &'static str },
    ResourceLimit { resource: Resource, limit: usize },
}

impl Error {
    /// Return the stable category of this error.
    #[must_use]
    pub const fn code(&self) -> ErrorCode {
        match self.kind {
            ErrorKind::Parse { .. } => ErrorCode::SqlParse,
            ErrorKind::Unsupported { .. } => ErrorCode::UnsupportedSql,
            ErrorKind::Schema(_) => ErrorCode::Schema,
            ErrorKind::Type(_) => ErrorCode::Type,
            ErrorKind::Constraint(_) => ErrorCode::Constraint,
            ErrorKind::CorruptStorage { .. } => ErrorCode::CorruptStorage,
            ErrorKind::RegexCompile(_) => ErrorCode::RegexCompile,
            ErrorKind::RegexRuntime(_) => ErrorCode::RegexRuntime,
            ErrorKind::ResourceLimit { .. } => ErrorCode::ResourceLimit,
            ErrorKind::Allocation { .. } => ErrorCode::Allocation,
            ErrorKind::Capacity { .. } => ErrorCode::Capacity,
        }
    }

    /// Return the location in the original SQL input, when available.
    ///
    /// Spans use half-open UTF-8 byte offsets. Semantic errors currently do
    /// not retain source locations and therefore return `None`.
    #[must_use]
    pub const fn span(&self) -> Option<Span> {
        match self.kind {
            ErrorKind::Parse { span, .. } | ErrorKind::Unsupported { span, .. } => Some(span),
            _ => None,
        }
    }

    /// Return a byte offset in the encoded database blob, when available.
    ///
    /// Some diagnostics identify the exact offending byte; structural record
    /// errors may identify the start of the containing record instead.
    #[must_use]
    pub const fn storage_offset(&self) -> Option<usize> {
        match self.kind {
            ErrorKind::CorruptStorage { offset, .. } => Some(offset),
            _ => None,
        }
    }

    /// Return the configured resource that was exceeded, when available.
    ///
    /// This is `Some` exactly when [`Error::limit`] is `Some`.
    #[must_use]
    pub const fn resource(&self) -> Option<Resource> {
        match self.kind {
            ErrorKind::ResourceLimit { resource, .. } => Some(resource),
            _ => None,
        }
    }

    /// Return the configured limit that was exceeded, when available.
    ///
    /// This is `Some` exactly when [`Error::resource`] is `Some`.
    #[must_use]
    pub const fn limit(&self) -> Option<usize> {
        match self.kind {
            ErrorKind::ResourceLimit { limit, .. } => Some(limit),
            _ => None,
        }
    }

    pub(crate) fn parse(message: impl Into<String>, span: Span) -> Self {
        Self::new(ErrorKind::Parse {
            message: message.into(),
            span,
        })
    }

    pub(crate) fn unsupported(feature: impl Into<String>, span: Span) -> Self {
        Self::new(ErrorKind::Unsupported {
            feature: feature.into(),
            span,
        })
    }

    pub(crate) fn schema(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Schema(message.into()))
    }

    pub(crate) fn type_error(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Type(message.into()))
    }

    pub(crate) fn constraint(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Constraint(message.into()))
    }

    pub(crate) fn corrupt_storage(offset: usize, message: impl Into<String>) -> Self {
        Self::new(ErrorKind::CorruptStorage {
            offset,
            message: message.into(),
        })
    }

    pub(crate) fn regex_compile(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::RegexCompile(message.into()))
    }

    pub(crate) fn regex_runtime(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::RegexRuntime(message.into()))
    }

    pub(crate) const fn allocation(operation: &'static str) -> Self {
        Self::new(ErrorKind::Allocation { operation })
    }

    pub(crate) const fn capacity(operation: &'static str) -> Self {
        Self::new(ErrorKind::Capacity { operation })
    }

    pub(crate) const fn resource_limit(resource: Resource, limit: usize) -> Self {
        Self::new(ErrorKind::ResourceLimit { resource, limit })
    }

    const fn new(kind: ErrorKind) -> Self {
        Self { kind }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            ErrorKind::Parse { message, span } => write!(
                formatter,
                "SQL parse error at bytes {}..{}: {message}",
                span.start(),
                span.end()
            ),
            ErrorKind::Unsupported { feature, span } => write!(
                formatter,
                "unsupported SQL feature at bytes {}..{}: {feature}",
                span.start(),
                span.end()
            ),
            ErrorKind::Schema(message) => write!(formatter, "schema error: {message}"),
            ErrorKind::Type(message) => write!(formatter, "type error: {message}"),
            ErrorKind::Constraint(message) => {
                write!(formatter, "constraint violation: {message}")
            }
            ErrorKind::CorruptStorage { offset, message } => {
                write!(formatter, "corrupt database at byte {offset}: {message}")
            }
            ErrorKind::RegexCompile(message) => {
                write!(
                    formatter,
                    "generated regex could not be compiled: {message}"
                )
            }
            ErrorKind::RegexRuntime(message) => {
                write!(formatter, "regex execution failed: {message}")
            }
            ErrorKind::Allocation { operation } => {
                write!(formatter, "allocation failed while {operation}")
            }
            ErrorKind::Capacity { operation } => {
                write!(formatter, "capacity exceeded while {operation}")
            }
            ErrorKind::ResourceLimit { resource, limit } => {
                write!(
                    formatter,
                    "resource limit exceeded for {resource} (limit: {limit})"
                )
            }
        }
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut diagnostic = formatter.debug_struct("Error");
        diagnostic.field("code", &self.code());
        if let Some(span) = self.span() {
            diagnostic.field("span", &span);
        }
        if let Some(offset) = self.storage_offset() {
            diagnostic.field("storage_offset", &offset);
        }
        if let Some(resource) = self.resource() {
            diagnostic.field("resource", &resource);
        }
        if let Some(limit) = self.limit() {
            diagnostic.field("limit", &limit);
        }
        match &self.kind {
            ErrorKind::Parse { message, .. }
            | ErrorKind::Schema(message)
            | ErrorKind::Type(message)
            | ErrorKind::Constraint(message)
            | ErrorKind::CorruptStorage { message, .. }
            | ErrorKind::RegexCompile(message)
            | ErrorKind::RegexRuntime(message) => {
                diagnostic.field("detail", message);
            }
            ErrorKind::Unsupported { feature, .. } => {
                diagnostic.field("detail", feature);
            }
            ErrorKind::Allocation { operation } | ErrorKind::Capacity { operation } => {
                diagnostic.field("operation", operation);
            }
            ErrorKind::ResourceLimit { .. } => {}
        }
        diagnostic.finish()
    }
}

impl std::error::Error for Error {}

/// A result returned by the varchar core.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests;
