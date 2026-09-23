import { useCallback, useEffect, useMemo, useRef, useState } from "preact/hooks";

import { Console } from "./components/Console.jsx";
import { LogPane } from "./components/LogPane.jsx";
import { ResultPane } from "./components/ResultPane.jsx";
import { ScanPane } from "./components/ScanPane.jsx";
import { StringDock } from "./components/StringDock.jsx";
import { Tape } from "./components/Tape.jsx";
import { Topbar } from "./components/Topbar.jsx";
import { AboutDialog, ImportDialog, PresetsDrawer, ReferenceDrawer } from "./components/dialogs.jsx";
import { Banner } from "./components/ui.jsx";
import { byteLength, encode } from "./lib/bytes.js";
import { createDb, describe, exec, load, splitStatements } from "./lib/db.js";
import { csvToStatements } from "./lib/csv.js";
import { useMountTransition } from "./lib/transition.js";
import { DEMO } from "./lib/presets.js";
import { readScan, splitRecords } from "./lib/tape.js";

const CSV_ROW_LIMIT = 500;
const FIRST_QUERY = "SELECT name, email FROM users WHERE active = TRUE";

const SCAN_PLACEHOLDER = {
  title: "No scan yet",
  body: "Run a SELECT and the pattern the planner compiled appears here, with every byte it matched highlighted in the string below.",
};
const RESULT_PLACEHOLDER = { title: "Nothing run yet", body: "Rows, affected counts and errors land here." };

export function App() {
  const db = useRef(null);
  const nextId = useRef(0);

  const [booted, setBooted] = useState(false);
  const [bootError, setBootError] = useState(null);
  const [blob, setBlob] = useState("");
  const [outcome, setOutcome] = useState(null);
  const [scan, setScan] = useState(null);
  const [blobBefore, setBlobBefore] = useState(null);
  const [explain, setExplain] = useState(true);
  const [current, setCurrent] = useState(0);
  const [entries, setEntries] = useState([]);
  const [sql, setSql] = useState(FIRST_QUERY);
  const [logOpen, setLogOpen] = useState(false);
  const [dockOpen, setDockOpen] = useState(true);
  const [dialog, setDialog] = useState("about");
  const [scanPlaceholder, setScanPlaceholder] = useState(SCAN_PLACEHOLDER);
  const [resultPlaceholder, setResultPlaceholder] = useState(RESULT_PLACEHOLDER);
  // Each run remounts the tape's track so the head crosses it once more. The
  // pointed row is the result row under the pointer or focus, if any.
  const [runId, setRunId] = useState(0);
  const [pointed, setPointed] = useState(null);

  const write = useCallback((entry) => {
    nextId.current += 1;
    setEntries((previous) => [
      ...previous,
      { id: nextId.current, time: new Date().toLocaleTimeString([], { hour12: false }), ...entry },
    ]);
  }, []);

  // The engine is booted exactly once for the lifetime of the tab.
  useEffect(() => {
    createDb()
      .then((instance) => {
        db.current = instance;
        setBlob(instance.dump());
        setBooted(true);
        write({ text: "varchar engine ready. The empty database is the three bytes in the dock", tone: "ok" });
      })
      .catch((error) => setBootError(String(error)));
  }, [write]);

  const logStatement = useCallback(
    (statement, envelope) =>
      write({
        statement: true,
        text: statement.replace(/\s+/g, " "),
        tone: envelope.ok ? "ok" : "err",
        detail: envelope.ok ? `→ ${describe(envelope)}` : `✗ ${envelope.error.message}`,
      }),
    [write],
  );

  /** Applies one envelope to the panes: string, highlights, scan and result. */
  const settle = useCallback((statement, envelope) => {
    setBlob(envelope.blob);
    // A mutation's ranges index the string it read, so that one is kept
    // alongside the live blob for as long as its scan is on screen.
    setBlobBefore(envelope.ok ? (envelope.blobBefore ?? null) : null);
    setOutcome({ statement, envelope });
    setScan(envelope.ok ? (envelope.scan ?? null) : null);
    setCurrent(0);
    setRunId((id) => id + 1);
    setPointed(null);
    if (envelope.ok && !envelope.scan) {
      // A statement with no scan leaves nothing to highlight, so the pattern
      // that drew the previous highlights cannot stay on screen either.
      setScanPlaceholder({
        title: "No scan for this statement",
        body: "Only a SELECT compiles to a pattern. Run one and every byte it matches lights up in the string below.",
      });
    } else if (!envelope.ok) {
      setScanPlaceholder({ title: "No scan", body: "The statement was rejected before anything was scanned." });
    }
  }, []);

  /** Runs statements in order, stopping at the first failure. */
  const run = useCallback(
    (input, { note } = {}) => {
      const statements = Array.isArray(input) ? input : splitStatements(input);
      if (statements.length === 0) return null;
      if (note) write({ text: note, tone: "note" });

      let last = null;
      for (const statement of statements) {
        const envelope = exec(db.current, statement);
        logStatement(statement, envelope);
        last = { statement, envelope };
        if (!envelope.ok) break;
      }
      settle(last.statement, last.envelope);
      return last;
    },
    [logStatement, settle, write],
  );

  const onDrop = useCallback(() => {
    db.current.reset();
    setBlob(db.current.dump());
    setOutcome(null);
    setScan(null);
    setCurrent(0);
    setPointed(null);
    setScanPlaceholder(SCAN_PLACEHOLDER);
    setResultPlaceholder({ title: "Empty", body: "Nothing left. Seed the demo data to start again." });
    write({ text: "database dropped, back to the three-byte header", tone: "note" });
  }, [write]);

  const onLoadBlob = useCallback(
    (text) => {
      const envelope = load(db.current, text.trim());
      setBlob(envelope.blob);
      setScan(null);
      setCurrent(0);
      setPointed(null);
      if (envelope.ok) {
        write({ text: `loaded ${byteLength(text.trim())} bytes into the database`, tone: "ok" });
        setDialog(null);
        setScanPlaceholder({
          title: "No scan yet",
          body: "Run a SELECT against the imported data to see its pattern.",
        });
      } else {
        write({ text: `import rejected: ${envelope.error.message}`, tone: "err" });
      }
      setOutcome({ statement: text.slice(0, 200), envelope });
    },
    [write],
  );

  const onImportCsv = useCallback(
    (text, fileName) => {
      let plan;
      try {
        plan = csvToStatements(text, fileName, CSV_ROW_LIMIT);
      } catch (error) {
        write({ text: `CSV import failed: ${error.message}`, tone: "err" });
        return;
      }
      const types = plan.columns.map((column) => `${column.name} ${column.type}`).join(", ");
      write({ text: `CSV → table ${plan.table} (${types})`, tone: "note" });
      if (plan.skipped > 0) {
        write({
          text: `only the first ${CSV_ROW_LIMIT} rows were imported; ${plan.skipped} skipped`,
          tone: "note",
        });
      }
      setDialog(null);
      run(plan.statements);
      // The generated DDL and inserts are in the log; leave the console holding
      // something the reader can actually run against the new table.
      setSql(`SELECT * FROM ${plan.table}`);
    },
    [run, write],
  );

  const onSave = useCallback(() => {
    const url = URL.createObjectURL(new Blob([blob], { type: "text/plain" }));
    const anchor = document.createElement("a");
    anchor.href = url;
    anchor.download = "playground.varchar";
    anchor.click();
    URL.revokeObjectURL(url);
  }, [blob]);

  const log = useMountTransition(logOpen);

  const stats = useMemo(
    () => ({
      bytes: byteLength(blob),
      tables: (blob.match(/~S\|/g) || []).length,
      rows: (blob.match(/~R\|/g) || []).length,
    }),
    [blob],
  );

  // The tape reads the same string the dock shows: a mutation's scan indexes
  // the string before the write, so its matches are drawn over that one or
  // not at all.
  const beforeWrite = scan?.appliesTo === "before";
  const historic = beforeWrite && typeof blobBefore === "string";
  const shown = historic && explain ? blobBefore : blob;
  const tapeScan = explain && (historic || !beforeWrite) ? scan : null;
  const envelope = outcome?.envelope;

  const reading = useMemo(() => {
    const bytes = encode(shown);
    const records = splitRecords(bytes);
    const result = tapeScan && envelope?.ok && envelope.kind === "rows" ? envelope.result : null;
    const { state, rows } = readScan(records, tapeScan, result);
    let litBytes = 0;
    records.forEach((record, index) => {
      if (state[index] === "lit" || state[index] === "half") litBytes += record.end - record.at;
    });
    return { records, state, rows, scan: tapeScan, total: bytes.length, litBytes };
  }, [shown, tapeScan, envelope]);

  if (bootError) {
    return (
      <div class="app-shell" style={{ padding: "24px" }}>
        <Banner tone="red">The varchar engine failed to load: {bootError}</Banner>
      </div>
    );
  }

  return (
    <div class="app-shell is-fixed">
      <Topbar
        stats={stats}
        logOpen={logOpen}
        onToggleLog={() => setLogOpen((open) => !open)}
        onOpenPresets={() => setDialog("presets")}
        onOpenReference={() => setDialog("reference")}
        onOpenImport={() => setDialog("import")}
        onOpenAbout={() => setDialog("about")}
      />

      <main class="workbench">
        <Console
          sql={sql}
          onSql={setSql}
          onRun={() => run(sql)}
          onSeed={() => {
            run(DEMO, { note: "seeding the demo schema and data" });
            setSql(FIRST_QUERY);
          }}
        />
        <ScanPane scan={scan} placeholder={scanPlaceholder} />
        <Tape reading={reading} runId={runId} pointed={(pointed !== null && reading.rows?.[pointed]) || []} />
        <ResultPane
          outcome={booted ? outcome : null}
          placeholder={resultPlaceholder}
          onPoint={reading.rows ? setPointed : null}
        />
      </main>

      {log.mounted ? (
        <LogPane
          leaving={log.leaving}
          entries={entries}
          onClear={() => setEntries([])}
          onClose={() => setLogOpen(false)}
        />
      ) : null}

      <StringDock
        blob={blob}
        blobBefore={blobBefore}
        scan={scan}
        explain={explain}
        onExplain={() => {
          setDockOpen(true);
          setCurrent(0);
          setExplain((on) => !on);
        }}
        current={current}
        onCurrent={(index) => {
          setDockOpen(true);
          setCurrent(index);
        }}
        open={dockOpen}
        onToggle={() => setDockOpen((open) => !open)}
        onSave={onSave}
        onDrop={onDrop}
      />

      <AboutDialog open={dialog === "about"} onClose={() => setDialog(null)} />
      <PresetsDrawer
        open={dialog === "presets"}
        onClose={() => setDialog(null)}
        onPick={(preset) => {
          // The console is loaded, not fired: the reader presses run.
          setSql(preset.sql.join(";\n"));
          setDialog(null);
        }}
      />
      <ReferenceDrawer
        open={dialog === "reference"}
        onClose={() => setDialog(null)}
        onUse={(statement) => {
          setSql(statement);
          setDialog(null);
        }}
      />
      <ImportDialog
        open={dialog === "import"}
        onClose={() => setDialog(null)}
        onLoadBlob={onLoadBlob}
        onImportCsv={onImportCsv}
      />
    </div>
  );
}
