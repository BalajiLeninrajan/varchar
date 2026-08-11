use super::Parser;
use crate::Result;
use crate::sql::ast::{DescribeTable, Statement};

impl Parser {
    pub(super) fn parse_show(&mut self) -> Result<Statement> {
        self.expect_keyword("SHOW")?;
        self.expect_keyword("TABLES")?;
        Ok(Statement::ShowTables)
    }

    pub(super) fn parse_describe_table(&mut self) -> Result<DescribeTable> {
        self.expect_keyword("DESCRIBE")?;
        Ok(DescribeTable {
            table: self.expect_identifier()?,
        })
    }
}
