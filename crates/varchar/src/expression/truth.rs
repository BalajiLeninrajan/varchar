//! SQL three-valued truth tables.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Truth {
    True,
    False,
    Unknown,
}

impl Truth {
    pub(super) const fn and(self, right: Self) -> Self {
        match (self, right) {
            (Self::False, _) | (_, Self::False) => Self::False,
            (Self::True, value) | (value, Self::True) => value,
            (Self::Unknown, Self::Unknown) => Self::Unknown,
        }
    }

    pub(super) const fn passes_where(self) -> bool {
        matches!(self, Self::True)
    }
}
