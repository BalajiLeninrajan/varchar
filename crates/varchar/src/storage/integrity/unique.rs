//! Bounded single-column UNIQUE validation indexes.

use std::cmp::Ordering;
use std::ops::Range;

use super::{ValidationError, ValidationResult, Violation};
use crate::Error;
use crate::limits::ByteBudget;
use crate::storage::decode::row_records;
use crate::storage::{Catalog, TableSchema};

const VALUES_OPERATION: &str = "reserving UNIQUE validation values";
const INDEXES_OPERATION: &str = "reserving UNIQUE validation indexes";

/// A source offset narrowed to the width the blob it points into needs.
trait SourceOffset: Copy {
    fn narrow(offset: usize) -> Self;

    fn widen(self) -> usize;
}

impl SourceOffset for u32 {
    fn narrow(offset: usize) -> Self {
        Self::try_from(offset).expect("an offset into a u32-sized database fits in u32")
    }

    fn widen(self) -> usize {
        usize::try_from(self).expect("a stored source offset originated as usize")
    }
}

impl SourceOffset for usize {
    fn narrow(offset: usize) -> Self {
        offset
    }

    fn widen(self) -> usize {
        self
    }
}

/// The bits every value below `bound` fits in.
fn bits_below(bound: usize) -> u32 {
    usize::BITS - bound.leading_zeros()
}

/// Where the single fill pass keeps the UNIQUE values it reads.
///
/// [`Self::Tagged`] is one flat vector holding every value with the index that owns it packed
/// above its source offset, so declaring a UNIQUE column costs nothing until the column holds a
/// value. Small databases need exactly that: a vector per index charges a header for every
/// UNIQUE column the schema declares, which is working memory owed before the first row is
/// read, and the exactly sized allocation this replaced never owed it — that fixed cost is what
/// closed column-heavy databases out of the working limit their own size derives.
///
/// A tag only fits beside an offset while the two share a `u32`. Past that the schema declares
/// so many indexes that the blob spends at least fourteen bytes on each of them — fifty-six
/// bytes of derived working limit — and a vector per index is affordable again, so the other
/// two layouts give every index its own and pay no tag at all.
enum UniqueValues {
    Tagged { offset_bits: u32, values: Vec<u32> },
    NarrowPerIndex(Vec<Vec<u32>>),
    WidePerIndex(Vec<Vec<usize>>),
}

impl UniqueValues {
    /// Chooses the layout for this database and charges what it owes before any row is read.
    ///
    /// Returns the layout beside the bytes it charged, so the pass that owns it hands the budget
    /// back exactly what it took.
    fn new(
        blob_len: usize,
        index_count: usize,
        budget: &mut ByteBudget,
    ) -> Result<(Self, usize), Error> {
        let offset_bits = bits_below(blob_len);
        if bits_below(index_count) + offset_bits <= u32::BITS {
            return Ok((
                Self::Tagged {
                    offset_bits,
                    values: Vec::new(),
                },
                0,
            ));
        }
        if u32::try_from(blob_len.saturating_sub(1)).is_ok() {
            let (slots, charged) = per_index_slots(index_count, budget)?;
            return Ok((Self::NarrowPerIndex(slots), charged));
        }
        let (slots, charged) = per_index_slots(index_count, budget)?;
        Ok((Self::WidePerIndex(slots), charged))
    }

    /// Records one value, returning the working bytes it charged.
    ///
    /// The count is returned rather than recovered from the vectors afterwards, so charge and
    /// release come from one place instead of from a capacity the allocator was free to round
    /// up.
    fn push(
        &mut self,
        index: usize,
        offset: usize,
        budget: &mut ByteBudget,
    ) -> Result<usize, Error> {
        match self {
            Self::Tagged {
                offset_bits,
                values,
            } => {
                let tagged = u32::try_from((index << *offset_bits) | offset)
                    .expect("a tagged value fits the width its index and offset chose");
                budget.push_charged(values, tagged, VALUES_OPERATION)
            }
            Self::NarrowPerIndex(slots) => {
                budget.push_charged(&mut slots[index], u32::narrow(offset), VALUES_OPERATION)
            }
            Self::WidePerIndex(slots) => {
                budget.push_charged(&mut slots[index], usize::narrow(offset), VALUES_OPERATION)
            }
        }
    }

    /// The index and source offset of the earliest duplicate value, if the database holds one.
    fn earliest_duplicate(&mut self, blob: &str) -> Option<(usize, usize)> {
        match self {
            Self::Tagged {
                offset_bits,
                values,
            } => earliest_tagged_duplicate(values, *offset_bits, blob),
            Self::NarrowPerIndex(slots) => earliest_per_index_duplicate(slots, blob),
            Self::WidePerIndex(slots) => earliest_per_index_duplicate(slots, blob),
        }
    }
}

/// Reserves one empty vector per index, returning them beside the bytes they charged.
fn per_index_slots<T>(
    index_count: usize,
    budget: &mut ByteBudget,
) -> Result<(Vec<Vec<T>>, usize), Error> {
    let mut slots = Vec::new();
    let charged = budget.reserve_exact(&mut slots, index_count, INDEXES_OPERATION)?;
    slots.resize_with(index_count, Vec::new);
    Ok((slots, charged))
}

/// Groups the tagged values by index, orders each group, and reports the earliest repeat.
///
/// Comparing the tag first keeps two indexes' values from ever being compared as strings, so
/// one sort over everything costs what a sort per index costs. The offset tie-break then reports
/// the later occurrence of a colliding pair exactly as a per-index sort does, and the earliest
/// occurrence over every index is the earliest of their per-index earliests.
fn earliest_tagged_duplicate(
    values: &mut [u32],
    offset_bits: u32,
    blob: &str,
) -> Option<(usize, usize)> {
    let parts = |tagged: u32| {
        let tagged = usize::try_from(tagged).expect("a tagged value originated as usize");
        (
            tagged >> offset_bits,
            tagged & (usize::MAX >> (usize::BITS - offset_bits)),
        )
    };
    values.sort_unstable_by(|left, right| {
        let (left_index, left_offset) = parts(*left);
        let (right_index, right_offset) = parts(*right);
        left_index
            .cmp(&right_index)
            .then_with(|| compare_unique_values(blob, left_offset, right_offset))
            .then_with(|| left_offset.cmp(&right_offset))
    });
    values
        .windows(2)
        .filter_map(|pair| {
            let (left_index, left_offset) = parts(pair[0]);
            let (right_index, right_offset) = parts(pair[1]);
            (left_index == right_index
                && compare_encoded_cells(blob, left_offset, right_offset) == Ordering::Equal)
                .then_some((right_index, right_offset))
        })
        .min_by_key(|(_, offset)| *offset)
}

fn earliest_per_index_duplicate<T: SourceOffset>(
    slots: &mut [Vec<T>],
    blob: &str,
) -> Option<(usize, usize)> {
    slots
        .iter_mut()
        .enumerate()
        .filter_map(|(index, offsets)| {
            duplicate_occurrence(offsets, blob).map(|offset| (index, offset))
        })
        .min_by_key(|(_, offset)| *offset)
}

fn duplicate_occurrence<T: SourceOffset>(offsets: &mut [T], blob: &str) -> Option<usize> {
    offsets.sort_unstable_by(|left, right| {
        let left = left.widen();
        let right = right.widen();
        compare_unique_values(blob, left, right).then_with(|| left.cmp(&right))
    });
    offsets
        .windows(2)
        .filter_map(|pair| {
            let left = pair[0].widen();
            let right = pair[1].widen();
            (compare_encoded_cells(blob, left, right) == Ordering::Equal).then_some(right)
        })
        .min()
}

/// One table's UNIQUE indexes, named by the range of index ids its columns own.
struct UniqueTable<'a> {
    schema: &'a TableSchema,
    indexes: Range<usize>,
}

#[cfg(test)]
std::thread_local! {
    static COMPARED_CELL_BYTES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn compare_encoded_cells(blob: &str, left: usize, right: usize) -> Ordering {
    let left = &blob.as_bytes()[left..];
    let right = &blob.as_bytes()[right..];
    for (left, right) in left.iter().zip(right) {
        #[cfg(test)]
        COMPARED_CELL_BYTES.with(|count| count.set(count.get() + 1));
        match (is_cell_delimiter(*left), is_cell_delimiter(*right)) {
            (true, true) => return Ordering::Equal,
            (true, false) => return Ordering::Less,
            (false, true) => return Ordering::Greater,
            (false, false) => match left.cmp(right) {
                Ordering::Equal => {}
                ordering => return ordering,
            },
        }
    }
    unreachable!("validated encoded cells end at row delimiters")
}

fn is_cell_delimiter(byte: u8) -> bool {
    matches!(byte, b'|' | b';')
}

fn source_offset(blob: &str, value: &str) -> usize {
    (value.as_ptr() as usize)
        .checked_sub(blob.as_ptr() as usize)
        .expect("a decoded row cell is borrowed from the database blob")
}

fn compare_unique_values(blob: &str, left: usize, right: usize) -> Ordering {
    #[cfg(test)]
    super::super::budget::record_working_string_insert_comparison();
    compare_encoded_cells(blob, left, right)
}

fn table_index(tables: &[UniqueTable<'_>], table: &str) -> Option<usize> {
    tables
        .binary_search_by(|candidate| candidate.schema.name.as_str().cmp(table))
        .ok()
}

pub(super) fn validate<'a>(
    blob: &'a str,
    catalog: &'a Catalog,
    budget: &mut ByteBudget,
) -> ValidationResult<()> {
    let mut table_count = 0_usize;
    let mut index_count = 0_usize;
    for schema in catalog.schemas() {
        if schema.unique_columns.is_empty() {
            continue;
        }
        table_count = table_count.checked_add(1).ok_or(Error::Capacity {
            operation: "counting tables with UNIQUE validation indexes",
        })?;
        index_count =
            index_count
                .checked_add(schema.unique_columns.len())
                .ok_or(Error::Capacity {
                    operation: "counting UNIQUE validation indexes",
                })?;
    }
    if index_count == 0 {
        return Ok(());
    }

    let table_bytes = table_count
        .checked_mul(std::mem::size_of::<UniqueTable<'_>>())
        .ok_or(Error::Capacity {
            operation: "sizing tables with UNIQUE validation indexes",
        })?;

    let mut tables = Vec::new();
    budget.reserve_exact(
        &mut tables,
        table_count,
        "reserving tables with UNIQUE validation indexes",
    )?;
    let mut indexes = 0;
    for schema in catalog.schemas() {
        if schema.unique_columns.is_empty() {
            continue;
        }
        let start = indexes;
        indexes += schema.unique_columns.len();
        tables.push(UniqueTable {
            schema,
            indexes: start..indexes,
        });
    }
    tables.sort_unstable_by(|left, right| left.schema.name.cmp(&right.schema.name));

    // Every reservation and append reports the bytes it charged, and this is the ledger they
    // accumulate into: it is what the release below hands back, so charge and release come from
    // one place instead of from a capacity the allocator was free to round up.
    let (mut values, mut value_bytes) = match UniqueValues::new(blob.len(), index_count, budget) {
        Ok(values) => values,
        Err(error) => {
            budget.release(table_bytes);
            return Err(ValidationError::Storage(error));
        }
    };

    let result = (|| -> ValidationResult<()> {
        // The fill pass is spelled out rather than delegated to `for_each_row` because growing
        // the values can exhaust the working budget, which is a storage error and not a row
        // violation.
        for row in row_records(blob, catalog.row_start) {
            let row = row.map_err(ValidationError::Storage)?;
            let Some(table) = table_index(&tables, row.table()).map(|index| &tables[index]) else {
                continue;
            };
            let mut unique = 0;
            for (column, value) in row.cells().enumerate() {
                if unique == table.indexes.len() {
                    break;
                }
                if table.schema.unique_columns[unique] != column {
                    continue;
                }
                if value != "N" {
                    value_bytes += values.push(
                        table.indexes.start + unique,
                        source_offset(blob, value),
                        budget,
                    )?;
                }
                unique += 1;
            }
            if unique != table.indexes.len() {
                return Err(Violation::new(
                    row.range().start,
                    "UNIQUE cell is missing from a validated row",
                )
                .into());
            }
        }

        let Some((index, occurrence)) = values.earliest_duplicate(blob) else {
            return Ok(());
        };
        let table = tables
            .iter()
            .find(|table| table.indexes.contains(&index))
            .expect("every UNIQUE value belongs to one of the indexed tables");
        let column = table.schema.unique_columns[index - table.indexes.start];
        for row in row_records(blob, catalog.row_start) {
            let row = row.map_err(ValidationError::Storage)?;
            if row.table() != table.schema.name {
                continue;
            }
            let value = row
                .cells()
                .nth(column)
                .expect("validated rows contain every UNIQUE cell");
            if source_offset(blob, value) == occurrence {
                return Err(Violation::new(
                    row.range().start,
                    format!(
                        "duplicate UNIQUE value for table {:?} column {:?}",
                        table.schema.name, table.schema.columns[column].name
                    ),
                )
                .into());
            }
        }
        unreachable!("a duplicate UNIQUE occurrence belongs to a validated row")
    })();
    // The values hand back exactly what their reservations and appends charged, alongside the
    // table descriptors, before the CHECK pass reserves.
    drop(values);
    drop(tables);
    budget.release(table_bytes + value_bytes);
    result
}

#[cfg(test)]
mod tests {
    use std::cmp::Ordering;

    use super::{
        COMPARED_CELL_BYTES, UniqueTable, UniqueValues, compare_encoded_cells,
        earliest_per_index_duplicate, validate,
    };
    use crate::limits::ByteBudget;
    use crate::storage::budget::{reset_working_string_comparisons, working_string_comparisons};
    use crate::storage::validate::validate_and_catalog;
    use crate::{Error, Resource};

    /// The bytes one table with UNIQUE columns costs before it holds a value.
    const TABLE: usize = std::mem::size_of::<UniqueTable<'static>>();
    /// The bytes the first tagged value costs, growth reserving two before it lands.
    const FIRST_VALUE: usize = 2 * std::mem::size_of::<u32>();

    fn working_budget(bytes: usize) -> ByteBudget {
        ByteBudget::new(bytes, Resource::StorageWorkingBytes)
    }

    fn layout(blob_len: usize, index_count: usize) -> UniqueValues {
        UniqueValues::new(blob_len, index_count, &mut working_budget(usize::MAX))
            .expect("an unlimited budget reserves any layout")
            .0
    }

    /// The tagged layout covers every database whose length and index count fit a `u32` between
    /// them, and hands off to a vector per index rather than truncating a tag.
    #[cfg(target_pointer_width = "64")]
    #[test]
    fn tagged_values_give_way_to_per_index_vectors_when_a_tag_stops_fitting() {
        assert!(matches!(layout(1 << 30, 1), UniqueValues::Tagged { .. }));
        assert!(matches!(layout(1 << 25, 32), UniqueValues::Tagged { .. }));
        assert!(matches!(
            layout(1 << 30, 2),
            UniqueValues::NarrowPerIndex(_)
        ));
        assert!(matches!(
            layout(u32::MAX as usize + 1, 1),
            UniqueValues::NarrowPerIndex(_)
        ));
        assert!(matches!(
            layout(u32::MAX as usize + 2, 1),
            UniqueValues::WidePerIndex(_)
        ));
    }

    /// The regression this pins: declaring a UNIQUE column may not cost working bytes by
    /// itself.
    ///
    /// Every fixture below holds exactly one value however many columns it declares, so all of
    /// them have to load inside the same budget. A vector per index charges its header for every
    /// declared column instead — fixed cost a database owes before it holds any rows, which
    /// closed column-heavy databases out of the working limit their own size derives.
    #[test]
    fn declaring_a_unique_column_costs_nothing_until_it_holds_a_value() {
        for columns in 1..=8 {
            let mut blob = String::from("V3;~S|t");
            for column in 0..columns {
                blob.push_str(&format!("|c{column}:T:?"));
            }
            blob.push(';');
            for column in 0..columns {
                blob.push_str(&format!("~U|t|c{column};"));
            }
            blob.push_str("~R|t|Ta");
            for _ in 1..columns {
                blob.push_str("|N");
            }
            blob.push(';');

            let (_, catalog) =
                validate_and_catalog(&blob, usize::MAX).expect("the fixture is valid");
            let exact = TABLE + FIRST_VALUE;
            assert!(
                validate(&blob, &catalog, &mut working_budget(exact)).is_ok(),
                "{columns} declared UNIQUE columns holding one value cost {exact} bytes"
            );
            assert!(matches!(
                validate(&blob, &catalog, &mut working_budget(exact - 1)),
                Err(super::ValidationError::Storage(Error::ResourceLimit {
                    resource: Resource::StorageWorkingBytes,
                    limit,
                })) if limit == exact - 1
            ));
        }
    }

    /// The database from the regression report: one row cannot collide with itself, so opening
    /// it costs one table descriptor and the single value it holds.
    #[test]
    fn a_one_row_unique_table_charges_a_table_and_one_value() {
        const BLOB: &str = "V3;~S|t0|i:T:!|u:T:?;~P|t0|i;~U|t0|u;~R|t0|Ta|Ta;";

        let (_, catalog) =
            validate_and_catalog(BLOB, usize::MAX).expect("the one-row fixture is valid");
        let exact = TABLE + FIRST_VALUE;

        assert!(
            validate(BLOB, &catalog, &mut working_budget(exact)).is_ok(),
            "a lone UNIQUE value costs one append"
        );
        assert!(matches!(
            validate(BLOB, &catalog, &mut working_budget(exact - 1)),
            Err(super::ValidationError::Storage(Error::ResourceLimit {
                resource: Resource::StorageWorkingBytes,
                limit,
            })) if limit == exact - 1
        ));
    }

    #[test]
    fn encoded_comparison_stops_at_the_first_different_byte() {
        let mut blob = String::from("Ta");
        blob.push_str(&"x".repeat(100_000));
        blob.push(';');
        let right = blob.len();
        blob.push_str("Tb");
        blob.push_str(&"x".repeat(100_000));
        blob.push(';');

        COMPARED_CELL_BYTES.with(|count| count.set(0));
        assert_eq!(compare_encoded_cells(&blob, 0, right), Ordering::Less);
        COMPARED_CELL_BYTES.with(|count| assert_eq!(count.get(), 2));
    }

    #[test]
    fn unique_indexes_accept_the_exact_logical_budget_and_reject_one_under() {
        let blob = "V3;~S|t|value:T:?;~U|t|value;~R|t|Tone;~R|t|Ttwo;~R|t|Tsix;";
        let (_, catalog) =
            validate_and_catalog(blob, usize::MAX).expect("the UNIQUE fixture is valid");
        // Growth reserves two values and then three, so three values are charged exactly.
        let exact = TABLE + 3 * std::mem::size_of::<u32>();

        let mut exact_budget = working_budget(exact);
        assert!(
            validate(blob, &catalog, &mut exact_budget).is_ok(),
            "the exact UNIQUE index budget is sufficient"
        );
        assert!(
            exact_budget.charge(exact).is_ok(),
            "completed UNIQUE validation releases its temporary values"
        );
        assert!(matches!(
            validate(blob, &catalog, &mut working_budget(exact - 1)),
            Err(super::ValidationError::Storage(Error::ResourceLimit {
                resource: Resource::StorageWorkingBytes,
                limit,
            })) if limit == exact - 1
        ));
    }

    #[test]
    fn all_null_unique_indexes_need_only_a_table_descriptor() {
        for blob in [
            "V3;~S|t|value:T:?;~U|t|value;",
            "V3;~S|t|value:T:?;~U|t|value;~R|t|N;~R|t|N;",
            "V3;~S|t|a:T:?|b:T:?;~U|t|a;~U|t|b;~R|t|N|N;",
        ] {
            let (_, catalog) =
                validate_and_catalog(blob, usize::MAX).expect("the UNIQUE fixture is valid");
            assert!(
                validate(blob, &catalog, &mut working_budget(TABLE)).is_ok(),
                "NULL values need no stored value at all"
            );
            assert!(matches!(
                validate(blob, &catalog, &mut working_budget(TABLE - 1)),
                Err(super::ValidationError::Storage(Error::ResourceLimit {
                    resource: Resource::StorageWorkingBytes,
                    limit,
                })) if limit == TABLE - 1
            ));
        }
    }

    /// One sort over every index still reports the earliest collision in the blob, whichever
    /// index and table own it.
    #[test]
    fn the_earliest_duplicate_wins_across_indexes_and_tables() {
        let mut blob = String::from(
            "V3;~S|later|a:T:?|b:T:?;~U|later|a;~U|later|b;\
             ~S|earlier|c:T:?;~U|earlier|c;",
        );
        blob.push_str("~R|later|Tx|Tq;~R|earlier|Tz;");
        let earliest = blob.len();
        blob.push_str("~R|earlier|Tz;~R|later|Tx|Tq;");

        assert!(matches!(
            validate_and_catalog(&blob, usize::MAX),
            Err(Error::CorruptStorage { offset, message })
                if offset == earliest
                    && message == "duplicate UNIQUE value for table \"earlier\" column \"c\""
        ));
    }

    /// The per-index layout selects the same collision the tagged one does: the earliest
    /// repeat in the blob, reported against the index that owns it.
    #[test]
    fn per_index_vectors_select_the_same_earliest_duplicate() {
        let mut blob = String::from("V3;~S|t|a:T:?|b:T:?;~U|t|a;~U|t|b;");
        let mut cells = Vec::new();
        for (first, second) in [("Tq", "Tz"), ("Tz", "Tq"), ("Tq", "Tz")] {
            blob.push_str("~R|t|");
            cells.push(blob.len());
            blob.push_str(first);
            blob.push('|');
            cells.push(blob.len());
            blob.push_str(second);
            blob.push(';');
        }
        // Column "a" repeats "Tq" at the third row, one cell before "b" repeats "Tz".
        let mut slots = vec![
            vec![cells[0], cells[2], cells[4]],
            vec![cells[1], cells[3], cells[5]],
        ];
        assert_eq!(
            earliest_per_index_duplicate(&mut slots, &blob),
            Some((0, cells[4]))
        );
    }

    #[test]
    fn unique_validation_uses_indexed_duplicate_checks() {
        const ROW_COUNT: usize = 4_096;

        let mut blob = String::from("V3;~S|items|value:I:?;~U|items|value;");
        for value in 0..ROW_COUNT {
            blob.push_str(&format!("~R|items|I{value};"));
        }
        let duplicate_offset = blob.len();
        blob.push_str("~R|items|I0;");

        reset_working_string_comparisons();
        let error = validate_and_catalog(&blob, usize::MAX).expect_err("duplicate is rejected");
        let (insert_comparisons, lookup_comparisons) = working_string_comparisons();

        assert!(matches!(
            error,
            Error::CorruptStorage { offset, message }
                if offset == duplicate_offset
                    && message == "duplicate UNIQUE value for table \"items\" column \"value\""
        ));
        assert_eq!(lookup_comparisons, 0);
        assert!(
            insert_comparisons <= (ROW_COUNT + 1) * 16,
            "{} UNIQUE values required {insert_comparisons} duplicate comparisons",
            ROW_COUNT + 1
        );
    }
}
