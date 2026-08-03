//! Canonical serialization for metadata, rows, and typed cells.

mod metadata;

#[cfg(test)]
mod tests;

pub(super) use metadata::{
    encode_auto_increment_record, encode_table_metadata, measure_table_metadata,
};

use std::fmt::Write as _;
use std::marker::PhantomData;

use super::format::{
    ROW_PREFIX, allocation_error, encode_text_into, encode_text_with, encoded_text_len,
};
use super::{RowLayout, ValidatedRowLayout, validate_row_layout};
use crate::value::validate_value;
use crate::{DataType, Error, Result, SchemaColumn, Value};

type InvariantBrand<'brand> = PhantomData<fn(&'brand ()) -> &'brand ()>;

/// A statement-scoped encoder tied to one validated physical row layout.
///
/// The brand proves layout-session provenance only. Callers remain responsible
/// for stable values and measurement-to-row pairing within one session.
pub(crate) struct ValidatedRowEncoder<'layout, 'brand> {
    layout: ValidatedRowLayout<'layout>,
    _brand: InvariantBrand<'brand>,
}

/// An exact row length branded to one validated encoder session.
#[derive(Debug)]
pub(crate) struct MeasuredRowEncoding<'brand> {
    encoded_len: usize,
    _brand: InvariantBrand<'brand>,
}

const _: () =
    assert!(std::mem::size_of::<MeasuredRowEncoding<'static>>() == std::mem::size_of::<usize>());

impl MeasuredRowEncoding<'_> {
    #[cfg(test)]
    pub(crate) const fn encoded_len(&self) -> usize {
        self.encoded_len
    }
}

const STALE_ROW_MEASUREMENT_OPERATION: &str = "encoding a row from a stale measurement";

struct MeasuredRowBuffer {
    encoded: String,
    expected_len: usize,
}

impl MeasuredRowBuffer {
    fn new(expected_len: usize) -> Result<Self> {
        let mut encoded = String::new();
        encoded
            .try_reserve_exact(expected_len)
            .map_err(|_| allocation_error("reserving an encoded row"))?;
        Ok(Self {
            encoded,
            expected_len,
        })
    }

    fn push(&mut self, value: char) -> Result<()> {
        self.ensure_room(value.len_utf8())?;
        self.encoded.push(value);
        Ok(())
    }

    fn push_str(&mut self, value: &str) -> Result<()> {
        self.ensure_room(value.len())?;
        self.encoded.push_str(value);
        Ok(())
    }

    fn push_text(&mut self, value: &str) -> Result<()> {
        encode_text_with(value, |character| self.push(character))
    }

    fn finish(self) -> Result<String> {
        if self.encoded.len() != self.expected_len {
            return Err(stale_row_measurement());
        }
        debug_assert_eq!(self.encoded.len(), self.expected_len);
        Ok(self.encoded)
    }

    fn ensure_room(&self, additional: usize) -> Result<()> {
        let next = self
            .encoded
            .len()
            .checked_add(additional)
            .ok_or_else(stale_row_measurement)?;
        if next > self.expected_len {
            return Err(stale_row_measurement());
        }
        Ok(())
    }
}

impl std::fmt::Write for MeasuredRowBuffer {
    fn write_str(&mut self, value: &str) -> std::fmt::Result {
        self.push_str(value).map_err(|_| std::fmt::Error)
    }
}

/// Run one encoding session with a fresh invariant brand.
///
/// Measurements cannot escape or cross sessions, and any encoder able to
/// consume one retains the validated layout borrow.
pub(crate) fn with_validated_row_encoder<'layout, Output>(
    layout: ValidatedRowLayout<'layout>,
    use_encoder: impl for<'brand> FnOnce(ValidatedRowEncoder<'layout, 'brand>) -> Output,
) -> Output {
    use_encoder(ValidatedRowEncoder {
        layout,
        _brand: PhantomData,
    })
}

impl<'layout, 'brand> ValidatedRowEncoder<'layout, 'brand> {
    pub(crate) fn measure<'values>(
        &self,
        value_count: usize,
        value_at: impl Fn(usize) -> Option<&'values Value>,
    ) -> Result<MeasuredRowEncoding<'brand>> {
        let row_layout = self.layout.layout();
        if value_count != row_layout.columns.len() {
            return Err(Error::Type(format!(
                "table {:?} expects {} values, got {}",
                row_layout.table,
                row_layout.columns.len(),
                value_count
            )));
        }

        let mut encoded_len = checked_sum(
            [ROW_PREFIX.len(), row_layout.table.len(), 1],
            "sizing an encoded row",
        )?;
        for (index, column) in row_layout.columns.iter().enumerate() {
            let value = value_at(index).ok_or(Error::Capacity {
                operation: "reading a row value for encoding",
            })?;
            let cell_len = encoded_cell_len(value, column)?;
            encoded_len = encoded_len
                .checked_add(1)
                .and_then(|length| length.checked_add(cell_len))
                .ok_or(Error::Capacity {
                    operation: "sizing an encoded row",
                })?;
        }
        Ok(MeasuredRowEncoding {
            encoded_len,
            _brand: PhantomData,
        })
    }

    pub(crate) fn encode<'values>(
        &self,
        measured: MeasuredRowEncoding<'brand>,
        value_at: impl Fn(usize) -> Option<&'values Value>,
    ) -> Result<String> {
        let MeasuredRowEncoding {
            encoded_len,
            _brand: _,
        } = measured;
        let layout = self.layout.layout();
        let mut encoded = MeasuredRowBuffer::new(encoded_len)?;
        encoded.push_str(ROW_PREFIX)?;
        encoded.push_str(layout.table)?;
        for (index, column) in layout.columns.iter().enumerate() {
            let value = value_at(index).ok_or(Error::Capacity {
                operation: "reading a row value for encoding",
            })?;
            encoded.push('|')?;
            encode_cell_into_measured(value, column, &mut encoded)?;
        }
        encoded.push(';')?;
        encoded.finish()
    }
}

/// Encode a complete row record, including its terminator.
pub(crate) fn encode_row(values: &[Value], layout: RowLayout<'_>) -> Result<String> {
    encode_row_from(values.len(), layout, |index| values.get(index))
}

#[cfg(test)]
pub(crate) fn encoded_row_len_from<'values>(
    value_count: usize,
    layout: RowLayout<'_>,
    value_at: impl Fn(usize) -> Option<&'values Value>,
) -> Result<usize> {
    let layout = validate_row_layout(layout)?;
    with_validated_row_encoder(layout, |encoder| {
        Ok(encoder.measure(value_count, value_at)?.encoded_len())
    })
}

pub(crate) fn encode_row_from<'values>(
    value_count: usize,
    layout: RowLayout<'_>,
    value_at: impl Fn(usize) -> Option<&'values Value>,
) -> Result<String> {
    let layout = validate_row_layout(layout)?;
    with_validated_row_encoder(layout, |encoder| {
        let measured = encoder.measure(value_count, &value_at)?;
        encoder.encode(measured, value_at)
    })
}

/// Encode one typed cell in its canonical storage representation.
pub(crate) fn encode_cell(value: &Value, column: &SchemaColumn) -> Result<String> {
    let encoded_len = encoded_cell_len(value, column)?;
    let mut encoded = String::new();
    encoded
        .try_reserve_exact(encoded_len)
        .map_err(|_| allocation_error("reserving an encoded cell"))?;
    encode_cell_into(value, column, &mut encoded)?;
    debug_assert_eq!(encoded.len(), encoded_len);
    Ok(encoded)
}

fn encoded_cell_len(value: &Value, column: &SchemaColumn) -> Result<usize> {
    validate_value(value, column)?;
    match (value, column.data_type) {
        (Value::Null, _) => Ok(1),
        (Value::Text(value), DataType::Text) => {
            encoded_text_len(value)?
                .checked_add(1)
                .ok_or(Error::Capacity {
                    operation: "sizing an encoded TEXT cell",
                })
        }
        (Value::Integer(value), DataType::Integer) => signed_decimal_len(*value)
            .checked_add(1)
            .ok_or(Error::Capacity {
                operation: "sizing an encoded INTEGER cell",
            }),
        (Value::Boolean(_), DataType::Boolean) => Ok(2),
        _ => unreachable!("value validation guarantees the encoded type"),
    }
}

fn encode_cell_into(value: &Value, column: &SchemaColumn, encoded: &mut String) -> Result<()> {
    validate_value(value, column)?;
    match (value, column.data_type) {
        (Value::Null, _) => encoded.push('N'),
        (Value::Text(value), DataType::Text) => {
            encoded.push('T');
            encode_text_into(value, encoded);
        }
        (Value::Integer(value), DataType::Integer) => {
            encoded.push('I');
            // Writing to a String is infallible.
            let _ = write!(encoded, "{value}");
        }
        (Value::Boolean(value), DataType::Boolean) => {
            encoded.push_str(if *value { "B1" } else { "B0" });
        }
        _ => unreachable!("value validation guarantees the encoded type"),
    }
    Ok(())
}

fn encode_cell_into_measured(
    value: &Value,
    column: &SchemaColumn,
    encoded: &mut MeasuredRowBuffer,
) -> Result<()> {
    validate_value(value, column)?;
    match (value, column.data_type) {
        (Value::Null, _) => encoded.push('N')?,
        (Value::Text(value), DataType::Text) => {
            encoded.push('T')?;
            encoded.push_text(value)?;
        }
        (Value::Integer(value), DataType::Integer) => {
            encoded.push('I')?;
            write!(encoded, "{value}").map_err(|_| stale_row_measurement())?;
        }
        (Value::Boolean(value), DataType::Boolean) => {
            encoded.push_str(if *value { "B1" } else { "B0" })?;
        }
        _ => unreachable!("value validation guarantees the encoded type"),
    }
    Ok(())
}

const fn stale_row_measurement() -> Error {
    Error::Capacity {
        operation: STALE_ROW_MEASUREMENT_OPERATION,
    }
}

fn signed_decimal_len(value: i64) -> usize {
    let magnitude = value.unsigned_abs();
    let digits = if magnitude == 0 {
        1
    } else {
        magnitude.ilog10() as usize + 1
    };
    digits + usize::from(value.is_negative())
}

fn checked_sum(parts: impl IntoIterator<Item = usize>, operation: &'static str) -> Result<usize> {
    parts.into_iter().try_fold(0_usize, |total, part| {
        total.checked_add(part).ok_or(Error::Capacity { operation })
    })
}
