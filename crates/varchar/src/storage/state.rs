//! Authoritative storage blobs paired with their derived catalogs.

use super::budget::working_limit;
use super::validate::{validate_and_catalog, validate_candidate};
use super::{Candidate, Catalog};
use crate::Result;

/// The canonical empty database.
pub(crate) const EMPTY_BLOB: &str = super::format::HEADER;

/// One validated authoritative blob and the catalog derived from that exact blob.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StorageState {
    blob: String,
    catalog: Catalog,
}

impl StorageState {
    pub(crate) fn empty() -> Self {
        Self {
            blob: EMPTY_BLOB.to_owned(),
            catalog: Catalog::empty(),
        }
    }

    pub(crate) fn load(blob: String, max_database_bytes: usize) -> Result<Self> {
        let catalog = validate_and_catalog(&blob, working_limit(max_database_bytes))?;
        Ok(Self { blob, catalog })
    }

    pub(super) fn from_candidate(blob: String, max_database_bytes: usize) -> Result<Self> {
        let catalog = validate_candidate(&blob, working_limit(max_database_bytes))?;
        Ok(Self { blob, catalog })
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.blob
    }

    pub(crate) fn into_string(self) -> String {
        self.blob
    }

    pub(crate) fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    pub(crate) fn candidate(&self, max_bytes: usize) -> Result<Candidate<'_>> {
        Candidate::new(self, max_bytes)
    }
}
