//! Validation and logical decomposition of SQL `LIKE` patterns.

use crate::{Error, Result};

/// Logical atoms in a validated SQL `LIKE` pattern.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LikeAtom {
    AnySequence,
    AnyScalar,
    Literal(char),
}

pub(super) fn resolve_like_pattern(pattern: &str) -> Result<Vec<LikeAtom>> {
    let mut atoms = Vec::new();
    let mut characters = pattern.chars();
    while let Some(character) = characters.next() {
        match character {
            '%' => atoms.push(LikeAtom::AnySequence),
            '_' => atoms.push(LikeAtom::AnyScalar),
            '\\' => {
                let Some(escaped) = characters.next() else {
                    return Err(Error::type_error(String::from(
                        "LIKE pattern ends with an incomplete escape",
                    )));
                };
                if !matches!(escaped, '%' | '_' | '\\') {
                    return Err(Error::type_error(format!(
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
