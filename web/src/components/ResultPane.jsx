import { Banner, Chip, EmptyState, Pane, PaneHead } from "./ui.jsx";
import { decodeRange, encode } from "../lib/bytes.js";

const CELL = {
  null: () => ({ className: "is-null", text: "NULL" }),
  boolean: (value) => ({ className: "is-boolean", text: value.v ? "TRUE" : "FALSE" }),
  integer: (value) => ({ className: "is-integer", text: value.v }),
  text: (value) => ({ className: "", text: value.v }),
};

function Table({ columns, rows }) {
  return (
    <table>
      <thead>
        <tr>
          {columns.map((column, index) => (
            <th key={index}>
              {column.label}
              <small>
                {column.type}
                {column.nullable ? "" : " NOT NULL"}
              </small>
            </th>
          ))}
        </tr>
      </thead>
      <tbody>
        {rows.map((row, rowIndex) => (
          <tr key={rowIndex}>
            {row.map((value, index) => {
              const cell = (CELL[value.t] ?? CELL.text)(value);
              return (
                <td key={index} class={cell.className} data-label={columns[index].label}>
                  {cell.text}
                </td>
              );
            })}
          </tr>
        ))}
      </tbody>
    </table>
  );
}

/** A parse or unsupported-syntax error carries a byte span into the input. */
function Offender({ statement, detail }) {
  const bytes = encode(statement);
  return (
    <pre class="well pattern-well scroll-well">
      {decodeRange(bytes, 0, detail.start)}
      <mark>{decodeRange(bytes, detail.start, detail.end) || "⟨here⟩"}</mark>
      {decodeRange(bytes, detail.end, bytes.length)}
    </pre>
  );
}

function ErrorView({ statement, error }) {
  return (
    <div class="scan-body" style={{ padding: "9px" }}>
      <Banner tone="red">{error.message}</Banner>
      {error.detail && typeof error.detail.start === "number" ? (
        <Offender statement={statement} detail={error.detail} />
      ) : null}
      <ul class="scan-steps">
        <li>
          <span class="mark-solid" style={{ "--accent": "var(--green)" }}>
            ✓
          </span>
          <div>The database string is unchanged, byte for byte. A failed statement never touches it.</div>
        </li>
      </ul>
    </div>
  );
}

const DONE = {
  affected: (envelope) => [
    "committed",
    `${envelope.rows.toLocaleString()} row${envelope.rows === 1 ? "" : "s"} written. The string below was rewritten in full.`,
  ],
  created: (envelope) => [
    "created",
    `Table ${envelope.table} created. Its schema now lives in the string as a ~S record.`,
  ],
  explain: () => ["explained", "Pattern compiled. No rows were scanned — EXPLAIN REGEX stops at the plan."],
  loaded: () => ["loaded", "Database string loaded and validated."],
};

export function ResultPane({ outcome, placeholder }) {
  let chips = null;
  let body = <EmptyState title={placeholder.title}>{placeholder.body}</EmptyState>;

  if (outcome) {
    const { statement, envelope } = outcome;
    if (!envelope.ok) {
      chips = (
        <Chip tone="red">
          <b>{envelope.error.kind}</b> error
        </Chip>
      );
      body = <ErrorView statement={statement} error={envelope.error} />;
    } else if (envelope.kind === "rows") {
      const { columns, rows } = envelope.result;
      chips = (
        <>
          <Chip>
            <b>{rows.length.toLocaleString()}</b> row{rows.length === 1 ? "" : "s"}
          </Chip>
          <Chip>
            <b>{columns.length}</b> col{columns.length === 1 ? "" : "s"}
          </Chip>
        </>
      );
      body =
        rows.length === 0 ? (
          <EmptyState title="no rows">The scan ran and matched nothing that survived the filter.</EmptyState>
        ) : (
          <Table columns={columns} rows={rows} />
        );
    } else {
      const [title, detail] = (DONE[envelope.kind] ?? (() => ["done", ""]))(envelope);
      body = <EmptyState title={title}>{detail}</EmptyState>;
    }
  }

  return (
    <Pane className="result-pane" aria-labelledby="result-heading">
      <PaneHead title="result" id="result-heading">
        {chips}
      </PaneHead>
      <div class="pane-body is-flush scroll-well">{body}</div>
    </Pane>
  );
}
