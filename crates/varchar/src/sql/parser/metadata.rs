use super::Parser;
use crate::Result;
use crate::sql::ast::Statement;

impl Parser {
    pub(super) fn parse_show(&mut self) -> Result<Statement> {
        self.expect_keyword("SHOW")?;
        self.expect_keyword("TABLES")?;
        Ok(Statement::ShowTables)
    }
}
