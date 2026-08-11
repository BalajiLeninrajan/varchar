//! Validation and logical decomposition of SQL `LIKE` patterns.

use crate::{Error, Result};

/// Logical atoms in one validated SQL `LIKE` pattern.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LikeAtom {
    AnySequence,
    AnyScalar,
    Literal(char),
}

pub(crate) fn compile_pattern(pattern: &str) -> Result<Vec<LikeAtom>> {
    let mut atoms = Vec::new();
    atoms
        .try_reserve_exact(pattern.chars().count())
        .map_err(|_| Error::Allocation {
            operation: "reserving a resolved LIKE pattern",
        })?;

    let mut characters = pattern.chars();
    while let Some(character) = characters.next() {
        match character {
            '%' => atoms.push(LikeAtom::AnySequence),
            '_' => atoms.push(LikeAtom::AnyScalar),
            '\\' => {
                let Some(escaped) = characters.next() else {
                    return Err(Error::Type(String::from(
                        "LIKE pattern ends with an incomplete escape",
                    )));
                };
                if !matches!(escaped, '%' | '_' | '\\') {
                    return Err(Error::Type(format!(
                        "LIKE pattern contains unsupported escape \\{escaped}"
                    )));
                }
                atoms.push(LikeAtom::Literal(escaped));
            }
            literal => atoms.push(LikeAtom::Literal(literal)),
        }
    }
    Ok(atoms)
}
