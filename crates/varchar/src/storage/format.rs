//! The physical V2/V3 grammar shared by decoding and encoding.

use std::fmt::Write as _;
use std::ops::Range;

use crate::{DataType, Error, Result};

pub(super) const V2_HEADER: &str = "V2;";
pub(super) const V3_HEADER: &str = "V3;";
pub(super) const SCHEMA_PREFIX: &str = "~S|";
pub(super) const PRIMARY_KEY_PREFIX: &str = "~P|";
pub(super) const FOREIGN_KEY_PREFIX: &str = "~F|";
pub(super) const AUTO_INCREMENT_PREFIX: &str = "~A|";
pub(super) const DEFAULT_PREFIX: &str = "~D|";
pub(super) const UNIQUE_PREFIX: &str = "~U|";
pub(crate) const ROW_PREFIX: &str = "~R|";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FormatVersion {
    V2,
    V3,
}

impl FormatVersion {
    pub(super) const fn header(self) -> &'static str {
        match self {
            Self::V2 => V2_HEADER,
            Self::V3 => V3_HEADER,
        }
    }

    pub(super) const fn supports_extensions(self) -> bool {
        matches!(self, Self::V3)
    }
}

pub(super) fn decode_header(blob: &str) -> Result<FormatVersion> {
    if blob.starts_with(V2_HEADER) {
        Ok(FormatVersion::V2)
    } else if blob.starts_with(V3_HEADER) {
        Ok(FormatVersion::V3)
    } else {
        Err(corrupt(0, "expected canonical V2; or V3; header"))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RecordKind {
    Schema,
    PrimaryKey,
    ForeignKey,
    AutoIncrement,
    Default,
    Unique,
    Row,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RecordRef<'a> {
    pub(super) text: &'a str,
    pub(super) range: Range<usize>,
    pub(super) kind: RecordKind,
}

pub(super) struct RecordIter<'a> {
    blob: &'a str,
    offset: usize,
    failed: bool,
}

pub(super) fn records(blob: &str, version: FormatVersion) -> RecordIter<'_> {
    records_from(blob, version.header().len())
}

pub(super) fn records_from(blob: &str, offset: usize) -> RecordIter<'_> {
    RecordIter {
        blob,
        offset,
        failed: false,
    }
}

impl<'a> Iterator for RecordIter<'a> {
    type Item = Result<RecordRef<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed || self.offset == self.blob.len() {
            return None;
        }

        let start = self.offset;
        let Some(remaining) = self.blob.get(start..) else {
            self.failed = true;
            return Some(Err(corrupt(start, "record offset is outside the database")));
        };
        if !remaining.starts_with('~') {
            self.failed = true;
            return Some(Err(corrupt(
                start,
                "expected a schema, constraint metadata, or row record",
            )));
        }
        let Some(relative_end) = remaining.find(';') else {
            self.failed = true;
            return Some(Err(corrupt(start, "unterminated record")));
        };
        let end = start + relative_end + 1;
        self.offset = end;
        let text = &self.blob[start..end];
        let kind = if text.starts_with(SCHEMA_PREFIX) {
            RecordKind::Schema
        } else if text.starts_with(PRIMARY_KEY_PREFIX) {
            RecordKind::PrimaryKey
        } else if text.starts_with(FOREIGN_KEY_PREFIX) {
            RecordKind::ForeignKey
        } else if text.starts_with(AUTO_INCREMENT_PREFIX) {
            RecordKind::AutoIncrement
        } else if text.starts_with(DEFAULT_PREFIX) {
            RecordKind::Default
        } else if text.starts_with(UNIQUE_PREFIX) {
            RecordKind::Unique
        } else if text.starts_with(ROW_PREFIX) {
            RecordKind::Row
        } else {
            RecordKind::Unknown
        };
        Some(Ok(RecordRef {
            text,
            range: start..end,
            kind,
        }))
    }
}

pub(super) fn complete_record_body<'a>(
    record: &'a str,
    prefix: &str,
    offset: usize,
) -> Result<&'a str> {
    let body = record
        .strip_prefix(prefix)
        .ok_or_else(|| corrupt(offset, "unexpected record tag"))?
        .strip_suffix(';')
        .ok_or_else(|| corrupt(offset, "unterminated record"))?;
    if body.contains(';') {
        return Err(corrupt(offset, "raw semicolon inside record"));
    }
    Ok(body)
}

/// Identifiers in storage are already canonicalized lowercase ASCII.
pub(super) fn is_valid_identifier(identifier: &str) -> bool {
    let mut bytes = identifier.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    (first == b'_' || first.is_ascii_lowercase())
        && bytes.all(|byte| byte == b'_' || byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

const ENCODED_TEXT_LENGTH_OPERATION: &str = "measuring encoded text";

pub(crate) fn encoded_text_len(value: &str) -> Result<usize> {
    let mut encoded_len = 0_usize;
    for character in value.chars() {
        let character_len = if must_escape(character) {
            7
        } else {
            character.len_utf8()
        };
        encoded_len = encoded_len
            .checked_add(character_len)
            .ok_or(Error::Capacity {
                operation: ENCODED_TEXT_LENGTH_OPERATION,
            })?;
    }
    Ok(encoded_len)
}

pub(crate) fn encode_text_into(value: &str, encoded: &mut String) {
    for character in value.chars() {
        if must_escape(character) {
            encoded.push('%');
            // Writing to a String is infallible.
            let _ = write!(encoded, "{:06X}", character as u32);
        } else {
            encoded.push(character);
        }
    }
}

pub(super) fn scan_text(payload: &str, offset: usize, mut accept: impl FnMut(char)) -> Result<()> {
    let bytes = payload.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 7 > bytes.len() {
                return Err(corrupt(offset + index, "truncated text escape"));
            }
            let digits = &bytes[index + 1..index + 7];
            if !digits
                .iter()
                .all(|byte| byte.is_ascii_digit() || (b'A'..=b'F').contains(byte))
            {
                return Err(corrupt(offset + index, "malformed text escape"));
            }
            let digits = std::str::from_utf8(digits)
                .map_err(|_| corrupt(offset + index, "malformed text escape"))?;
            let scalar = u32::from_str_radix(digits, 16)
                .map_err(|_| corrupt(offset + index, "malformed text escape"))?;
            let character = char::from_u32(scalar)
                .ok_or_else(|| corrupt(offset + index, "escape is not a Unicode scalar"))?;
            if !must_escape(character) {
                return Err(corrupt(
                    offset + index,
                    "unnecessary noncanonical text escape",
                ));
            }
            accept(character);
            index += 7;
            continue;
        }

        let character = payload[index..]
            .chars()
            .next()
            .ok_or_else(|| corrupt(offset + index, "invalid UTF-8 text payload"))?;
        if must_escape(character) {
            return Err(corrupt(offset + index, "unescaped structural character"));
        }
        accept(character);
        index += character.len_utf8();
    }
    Ok(())
}

pub(super) fn type_tag(data_type: DataType) -> char {
    match data_type {
        DataType::Text => 'T',
        DataType::Integer => 'I',
        DataType::Boolean => 'B',
    }
}

fn must_escape(character: char) -> bool {
    matches!(character, '%' | '~' | '|' | ';' | '\u{2028}' | '\u{2029}') || character.is_control()
}

pub(super) fn corrupt(offset: usize, message: impl Into<String>) -> Error {
    Error::CorruptStorage {
        offset,
        message: message.into(),
    }
}

pub(super) const fn allocation_error(operation: &'static str) -> Error {
    Error::Allocation { operation }
}
