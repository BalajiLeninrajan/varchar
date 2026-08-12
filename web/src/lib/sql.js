// Tokenizer for displaying SQL in the reference: keywords, column types and
// literals each get their own colour, the same three the storage sample uses.
// Identifiers, operators and punctuation are deliberately left plain.

const KEYWORDS = new Set([
  "AND", "ASC", "AUTOINCREMENT", "AUTO_INCREMENT", "BY", "CASCADE", "CHECK", "CREATE", "DEFAULT",
  "DELETE", "DESC", "DESCRIBE", "EXPLAIN", "FOREIGN", "FROM", "IN", "INNER", "INSERT", "INTO", "IS",
  "JOIN", "KEY", "LIKE", "LIMIT", "NOT", "OFFSET", "ON", "OR", "ORDER", "PRIMARY", "REFERENCES",
  "REGEX", "RESTRICT", "SELECT", "SET", "SHOW", "TABLE", "TABLES", "UNIQUE", "UPDATE", "VALUES",
  "WHERE",
]);
const TYPES = new Set(["BOOLEAN", "INTEGER", "TEXT"]);
const VALUES = new Set(["FALSE", "NULL", "TRUE"]);

// A quoted string (apostrophes doubled inside), an integer, or a word.
const TOKEN = /('(?:[^']|'')*')|(\d+)|([A-Za-z_][A-Za-z0-9_]*)/g;

export function tokenizeSql(text) {
  const tokens = [];
  let plain = 0;
  const flush = (upto) => {
    if (upto > plain) tokens.push({ kind: "plain", text: text.slice(plain, upto) });
  };

  TOKEN.lastIndex = 0;
  for (let match = TOKEN.exec(text); match; match = TOKEN.exec(text)) {
    flush(match.index);
    const [word] = match;
    const upper = word.toUpperCase();
    let kind = "plain";
    if (match[1]) kind = "val";
    else if (match[2]) kind = "val";
    else if (KEYWORDS.has(upper)) kind = "kw";
    else if (TYPES.has(upper)) kind = "type";
    else if (VALUES.has(upper)) kind = "val";
    tokens.push({ kind, text: word });
    plain = match.index + word.length;
  }
  flush(text.length);
  return tokens;
}
