use thiserror::Error;

use crate::Resource;

/// A half-open byte range in the SQL input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub(crate) const fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }
}

/// Errors produced by the platform-neutral database core.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    #[error("SQL parse error at bytes {span_start}..{span_end}: {message}")]
    Parse {
        message: String,
        span_start: usize,
        span_end: usize,
    },

    #[error("unsupported SQL feature at bytes {span_start}..{span_end}: {feature}")]
    Unsupported {
        feature: String,
        span_start: usize,
        span_end: usize,
    },

    #[error("schema error: {0}")]
    Schema(String),

    #[error("type error: {0}")]
    Type(String),

    #[error("constraint violation: {0}")]
    Constraint(String),

    #[error("corrupt database at byte {offset}: {message}")]
    CorruptStorage { offset: usize, message: String },

    #[error("generated regex could not be compiled: {0}")]
    RegexCompile(String),

    #[error("regex execution failed: {0}")]
    RegexRuntime(String),

    #[error("allocation failed while {operation}")]
    Allocation { operation: &'static str },

    #[error("capacity exceeded while {operation}")]
    Capacity { operation: &'static str },

    #[error("resource limit exceeded for {resource} (limit: {limit})")]
    ResourceLimit { resource: Resource, limit: usize },
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

pub type Result<T> = std::result::Result<T, Error>;
