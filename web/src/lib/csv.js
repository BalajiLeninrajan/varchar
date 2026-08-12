// CSV -> varchar DDL/DML. Types are inferred per column, and everything the
// import decides is reported back so the log can show the generated SQL.

const INTEGER = /^-?\d+$/;
const BOOLEAN = /^(true|false)$/i;
const IDENTIFIER = /^[A-Za-z_][A-Za-z0-9_]*$/;
const I64_MIN = -(2n ** 63n);
const I64_MAX = 2n ** 63n - 1n;

/** Splits RFC 4180-ish CSV, honouring "" escapes inside quoted fields. */
export function parseCsv(text, delimiter = ",") {
  const rows = [];
  let row = [];
  let field = "";
  let quoted = false;
  let dirty = false;
  for (let index = 0; index < text.length; index += 1) {
    const character = text[index];
    if (quoted) {
      if (character !== '"') {
        field += character;
      } else if (text[index + 1] === '"') {
        field += '"';
        index += 1;
      } else {
        quoted = false;
      }
      continue;
    }
    if (character === '"') {
      quoted = true;
      dirty = true;
    } else if (character === delimiter) {
      row.push(field);
      field = "";
      dirty = false;
    } else if (character === "\n" || character === "\r") {
      if (character === "\r" && text[index + 1] === "\n") index += 1;
      row.push(field);
      rows.push(row);
      row = [];
      field = "";
      dirty = false;
    } else {
      field += character;
    }
  }
  if (field !== "" || dirty || row.length > 0) {
    row.push(field);
    rows.push(row);
  }
  return rows.filter((entry) => entry.length > 1 || entry[0] !== "");
}

function normalizeIdentifier(raw, fallback) {
  const cleaned = raw.trim().toLowerCase().replace(/[^a-z0-9_]+/g, "_").replace(/^_+|_+$/g, "");
  return IDENTIFIER.test(cleaned) ? cleaned : fallback;
}

function inferType(values) {
  const present = values.filter((value) => value !== "");
  if (present.length === 0) return "TEXT";
  if (present.every((value) => BOOLEAN.test(value))) return "BOOLEAN";
  if (present.every((value) => INTEGER.test(value) && fitsI64(value))) return "INTEGER";
  return "TEXT";
}

function fitsI64(value) {
  const number = BigInt(value);
  return number >= I64_MIN && number <= I64_MAX;
}

function literal(value, type) {
  if (value === "") return "NULL";
  if (type === "INTEGER") return value.replace(/^\+/, "");
  if (type === "BOOLEAN") return value.toLowerCase() === "true" ? "TRUE" : "FALSE";
  return `'${value.replace(/'/g, "''")}'`;
}

/**
 * Builds the statements that recreate a CSV file as a varchar table.
 * Returns the table name, the inferred columns, and the statements to run.
 */
export function csvToStatements(text, fileName, rowLimit = 500) {
  const rows = parseCsv(text, text.includes("\t") && !text.includes(",") ? "\t" : ",");
  if (rows.length < 2) {
    throw new Error("the file needs a header row and at least one data row");
  }
  const [header, ...body] = rows;
  const table = normalizeIdentifier(fileName.replace(/\.[^.]+$/, ""), "imported");
  const used = new Set();
  const columns = header.map((raw, index) => {
    let name = normalizeIdentifier(raw, `column_${index + 1}`);
    while (used.has(name)) name = `${name}_${index + 1}`;
    used.add(name);
    const values = body.map((entry) => (entry[index] ?? "").trim());
    return { name, type: inferType(values) };
  });

  const kept = body.slice(0, rowLimit);
  const statements = [
    `CREATE TABLE ${table} (${columns.map((column) => `${column.name} ${column.type}`).join(", ")})`,
  ];
  for (const entry of kept) {
    const cells = columns.map((column, index) => literal((entry[index] ?? "").trim(), column.type));
    statements.push(`INSERT INTO ${table} VALUES (${cells.join(", ")})`);
  }
  return { table, columns, statements, imported: kept.length, skipped: body.length - kept.length };
}
