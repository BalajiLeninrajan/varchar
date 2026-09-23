import { useCallback, useEffect, useMemo, useRef, useState } from "preact/hooks";

import { Answer, ReadHead } from "./components/Answer.jsx";
import { Ask } from "./components/Ask.jsx";
import { LogPane } from "./components/LogPane.jsx";
import { Printout } from "./components/Printout.jsx";
import { Tape } from "./components/Tape.jsx";
import { Footer, Topbar } from "./components/Topbar.jsx";
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

export function App() {
  const db = useRef(null);
  const nextId = useRef(0);

  const [booted, setBooted] = useState(false);
  const [bootError, setBootError] = useState(null);
  const [blob, setBlob] = useState("");
  const [outcome, setOutcome] = useState(null);
  const [blobBefore, setBlobBefore] = useState(null);
  const [showBefore, setShowBefore] = useState(true);
  const [entries, setEntries] = useState([]);
  const [sql, setSql] = useState(FIRST_QUERY);
  const [logOpen, setLogOpen] = useState(false);
  const [dialog, setDialog] = useState(null);
  // Each run remounts the tape so the head crosses it once more. The pointed
  // row starts on the first one, and the parked head only glides between
  // rows once the reader has pointed at one.
  const [runId, setRunId] = useState(0);
  const [pointed, setPointed] = useState(0);
  const [live, setLive] = useState(false);

  const write = useCallback((entry) => {
    nextId.current += 1;
    setEntries((previous) => [
      ...previous,
      { id: nextId.current, time: new Date().toLocaleTimeString([], { hour12: false }), ...entry },
    ]);
  }, []);

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

  /** Shows one envelope: the string, the scan, and the rows. */
  const settle = useCallback((statement, envelope) => {
    setBlob(envelope.blob);
    // A mutation's ranges index the string it read, so that one is kept
    // alongside the live blob for as long as its scan is on screen.
    setBlobBefore(envelope.ok ? (envelope.blobBefore ?? null) : null);
    setShowBefore(true);
    setOutcome({ statement, envelope });
    setRunId((id) => id + 1);
    setPointed(0);
    setLive(false);
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

  // The engine boots once per tab, seeded with the demo data and answering
  // the first example, so the first screen already shows a scan.
  useEffect(() => {
    createDb()
      .then((instance) => {
        db.current = instance;
        setBooted(true);
        write({ text: "varchar engine ready", tone: "ok" });
        run(DEMO, { note: "seeding the demo schema and data" });
        run(FIRST_QUERY);
      })
      .catch((error) => setBootError(String(error)));
  }, [write, run]);

  const onDrop = useCallback(() => {
    db.current.reset();
    setBlob(db.current.dump());
    setBlobBefore(null);
    setOutcome(null);
    setRunId((id) => id + 1);
    write({ text: "database dropped, back to the three-byte header", tone: "note" });
  }, [write]);

  const onLoadBlob = useCallback(
    (text) => {
      const envelope = load(db.current, text.trim());
      if (envelope.ok) {
        write({ text: `loaded ${byteLength(text.trim())} bytes into the database`, tone: "ok" });
        setDialog(null);
      } else {
        write({ text: `import rejected: ${envelope.error.message}`, tone: "err" });
      }
      settle(text.slice(0, 200), envelope);
    },
    [settle, write],
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
      // The generated DDL and inserts are in the log. Leave the input holding
      // something the reader can run against the new table.
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
  // The log opens under the printout, so bring it into view.
  useEffect(() => {
    if (logOpen) document.getElementById("log-pane")?.scrollIntoView({ block: "nearest" });
  }, [logOpen]);

  // What the tape shows. A mutation's scan indexes the string before the
  // write, so its highlights are drawn over that string or not at all.
  const envelope = outcome?.envelope;
  const rawScan = envelope?.ok ? (envelope.scan ?? null) : null;
  const beforeWrite = rawScan?.appliesTo === "before";
  const canShowBefore = beforeWrite && typeof blobBefore === "string";
  const historic = canShowBefore && showBefore;
  const shown = historic ? blobBefore : blob;
  const scan = beforeWrite && !historic ? null : rawScan;

  const reading = useMemo(() => {
    const bytes = encode(shown);
    const records = splitRecords(bytes);
    const result = envelope?.kind === "rows" ? envelope.result : null;
    const { state, rows } = readScan(records, scan, result);
    let lit = 0;
    let litBytes = 0;
    let tested = 0;
    records.forEach((record, index) => {
      if (state[index] === "lit" || state[index] === "half") {
        lit += 1;
        litBytes += record.end - record.at;
      }
      if (record.kind === "row" && scan?.sources.includes(record.table)) tested += 1;
    });
    return { records, state, rows, scan, total: bytes.length, lit, litBytes, tested, historic };
  }, [shown, scan, envelope, historic]);

  const hasDemo = blob.includes("~S|users|") && blob.includes("~S|posts|");

  if (bootError) {
    return (
      <div class="app-shell cn-p-24">
        <Banner tone="red">The varchar engine failed to load: {bootError}</Banner>
      </div>
    );
  }

  return (
    <div class="app-shell">
      <Topbar
        logOpen={logOpen}
        onToggleLog={() => setLogOpen((open) => !open)}
        onOpenReference={() => setDialog("reference")}
        onOpenImport={() => setDialog("import")}
        onOpenAbout={() => setDialog("about")}
      />

      <main class="page-main page-enter vc-main">
        <Ask
          sql={sql}
          onSql={setSql}
          onRun={() => run(sql)}
          hasDemo={hasDemo}
          onSeed={() => {
            run(DEMO, { note: "seeding the demo schema and data" });
            setSql(FIRST_QUERY);
          }}
          onExample={(statement) => {
            setSql(statement);
            run(statement);
          }}
          onMore={() => setDialog("presets")}
        />
        <Answer booted={booted} outcome={outcome} reading={reading} />
        <ReadHead outcome={outcome} reading={reading} />
        <Tape
          reading={reading}
          runId={runId}
          pointed={reading.rows?.[pointed] ?? []}
          live={live}
          historic={historic}
          onHistoric={canShowBefore ? () => setShowBefore((on) => !on) : null}
          blob={blob}
          onSave={onSave}
          onDrop={onDrop}
        />
        <Printout
          booted={booted}
          outcome={outcome}
          reading={reading}
          onPoint={(index) => {
            setLive(true);
            setPointed(index);
          }}
        />
        {log.mounted ? (
          <LogPane
            leaving={log.leaving}
            entries={entries}
            onClear={() => setEntries([])}
            onClose={() => setLogOpen(false)}
          />
        ) : null}
      </main>

      <Footer />

      <AboutDialog open={dialog === "about"} onClose={() => setDialog(null)} />
      <PresetsDrawer
        open={dialog === "presets"}
        onClose={() => setDialog(null)}
        onPick={(preset) => {
          // The input is loaded, not fired: the reader presses run.
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
