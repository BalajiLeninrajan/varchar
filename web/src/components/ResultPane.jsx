import { Banner, Chip, EmptyState, Pane, PaneHead } from "./ui.jsx";
import { decodeRange, encode } from "../lib/bytes.js";

const CELL = {
  null: () => ({ className: "is-null", text: "NULL" }),
  boolean: (value) => ({ className: "is-boolean", text: value.v ? "TRUE" : "FALSE" }),
  integer: (value) => ({ className: "is-integer", text: value.v }),
  text: (value) => ({ className: "", text: value.v }),
};

/** With `onPoint`, a row under the pointer or focus outlines its records on the tape. */
function Table({ columns, rows, onPoint }) {
  return (
    <table class="data-table is-sticky-head">
      <thead>
        <tr>
          {columns.map((column, index) => (
            <th key={index} class="cn-nowrap">
              {column.label}
              <small class="cn-microlabel cn-block cn-mt-4">
                {column.type}
                {column.nullable ? "" : " NOT NULL"}
              </small>
            </th>
          ))}
        </tr>
      </thead>
      <tbody>
        {rows.map((row, rowIndex) => (
          <tr
            key={rowIndex}
            tabindex={onPoint ? 0 : undefined}
            onMouseEnter={onPoint && (() => onPoint(rowIndex))}
            onMouseLeave={onPoint && (() => onPoint(null))}
            onFocus={onPoint && (() => onPoint(rowIndex))}
            onBlur={onPoint && (() => onPoint(null))}
          >
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
    <pre class="pattern-well codeblock scroll-well">
      {decodeRange(bytes, 0, detail.start)}
      <mark>{decodeRange(bytes, detail.start, detail.end) || "⟨here⟩"}</mark>
      {decodeRange(bytes, detail.end, bytes.length)}
    </pre>
  );
}

function ErrorView({ statement, error }) {
  return (
    <div class="cn-stack cn-p-12">
      <Banner tone="red">{error.message}</Banner>
      {error.detail && typeof error.detail.start === "number" ? (
        <Offender statement={statement} detail={error.detail} />
      ) : null}
      <p class="cn-meta cn-m-0">The database string is unchanged, byte for byte. A failed statement never touches it.</p>
    </div>
  );
}

const DONE = {
  affected: (envelope) => [
    "Committed",
    `${envelope.rows.toLocaleString()} row${envelope.rows === 1 ? "" : "s"} written. The string below was rewritten in full.`,
  ],
  created: (envelope) => [
    "Created",
    `Table ${envelope.table} created. Its schema now lives in the string as a ~S record.`,
  ],
  explain: () => ["Explained", "Pattern compiled. No rows were scanned, because EXPLAIN REGEX stops at the plan."],
  loaded: () => ["Loaded", "Database string loaded and validated."],
};

export function ResultPane({ outcome, placeholder, onPoint }) {
  let chips = null;
  let body = <EmptyState title={placeholder.title}>{placeholder.body}</EmptyState>;

  if (outcome) {
    const { statement, envelope } = outcome;
    if (!envelope.ok) {
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
          <EmptyState title="No rows">The scan ran and matched nothing that survived the filter.</EmptyState>
        ) : (
          <Table columns={columns} rows={rows} onPoint={onPoint} />
        );
    } else {
      const [title, detail] = (DONE[envelope.kind] ?? (() => ["Done", ""]))(envelope);
      body = <EmptyState title={title}>{detail}</EmptyState>;
    }
  }

  return (
    <Pane className="result-pane" aria-labelledby="result-heading">
      <PaneHead title="Result" id="result-heading">
        {chips}
      </PaneHead>
      <div class="pane-body scroll-well">{body}</div>
    </Pane>
  );
}
