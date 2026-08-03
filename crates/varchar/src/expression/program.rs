//! Flat resolved Boolean-expression programs.

use crate::Value;
use crate::resolve::ColumnLocation;

use super::like::LikeAtom;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Predicate<'statement> {
    Equal {
        column: ColumnLocation,
        value: &'statement Value,
    },
    NotEqual {
        column: ColumnLocation,
        value: &'statement Value,
    },
    Like {
        column: ColumnLocation,
        atoms: Vec<LikeAtom>,
    },
    IsNull {
        column: ColumnLocation,
    },
    IsNotNull {
        column: ColumnLocation,
    },
}

impl Predicate<'_> {
    pub(crate) const fn column(&self) -> ColumnLocation {
        match self {
            Self::Equal { column, .. }
            | Self::NotEqual { column, .. }
            | Self::Like { column, .. }
            | Self::IsNull { column }
            | Self::IsNotNull { column } => *column,
        }
    }
}
