use super::Parser;
use crate::sql::ast::{DescribeTable, ShowCreateTable, Statement};
use crate::{Error, Result};

impl Parser {
    pub(super) fn parse_show(&mut self) -> Result<Statement> {
        self.expect_keyword("SHOW")?;
        match self.current_word() {
            Some("TABLES") => {
                self.advance();
                Ok(Statement::ShowTables)
            }
            Some("CREATE") => {
                self.advance();
                self.expect_keyword("TABLE")?;
                Ok(Statement::ShowCreateTable(ShowCreateTable {
                    table: self.expect_identifier()?,
                }))
            }
            _ => Err(Error::parse(
                "expected TABLES or CREATE TABLE after SHOW",
                self.current().span,
            )),
        }
    }

    pub(super) fn parse_describe_table(&mut self) -> Result<DescribeTable> {
        self.expect_keyword("DESCRIBE")?;
        Ok(DescribeTable {
            table: self.expect_identifier()?,
        })
    }
}
