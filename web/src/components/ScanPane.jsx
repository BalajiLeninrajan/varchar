import { Banner, Chip, CopyButton, EmptyState, Pane, PaneHead } from "./ui.jsx";
import { tokenizePattern } from "../lib/pattern.js";

export function ScanPane({ scan, placeholder }) {
  if (!scan) {
    return (
      <Pane className="scan-pane" aria-labelledby="scan-heading">
        <PaneHead title="the regex" id="scan-heading" />
        <div class="pane-body scroll-well">
          <EmptyState title={placeholder.title}>{placeholder.body}</EmptyState>
        </div>
      </Pane>
    );
  }

  const matched = scan.matchCount ?? scan.matches?.length ?? 0;

  return (
    <Pane className="scan-pane" aria-labelledby="scan-heading">
      <PaneHead title="the regex" id="scan-heading">
        {scan.exact ? (
          <Chip tone="green" title="Every predicate is in the pattern: the matches are the result rows">
            <b>exact</b> filter
          </Chip>
        ) : (
          <Chip tone="peach" title="Residual predicates and JOIN ... ON are re-checked in Rust after the scan">
            <b>prefilter</b>
          </Chip>
        )}
        <Chip>
          scans <b>{scan.sources.join(", ")}</b>
        </Chip>
        <Chip>
          <b>{matched.toLocaleString()}</b> matches
        </Chip>
        <CopyButton text={scan.pattern} />
      </PaneHead>

      <div class="pane-body scroll-well">
        <pre class="pattern">
          {tokenizePattern(scan.pattern).map((token, index) => (
            <span key={index} class={`re-${token.kind}`}>
              {token.text}
            </span>
          ))}
        </pre>
        {scan.truncated ? <Banner>Only the first 4096 matches were collected for display.</Banner> : null}
        {scan.scanError ? <Banner>Replaying the pattern for display failed: {scan.scanError}</Banner> : null}
      </div>
    </Pane>
  );
}
