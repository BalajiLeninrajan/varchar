import { useEffect, useRef, useState } from "preact/hooks";

export function Icon({ id, size = 12 }) {
  return (
    <svg width={size} height={size} aria-hidden="true">
      <use href={`#i-${id}`} />
    </svg>
  );
}

export function Pane({ className = "", children, ...rest }) {
  return (
    <section class={`pane panel ${className}`.trim()} {...rest}>
      {children}
    </section>
  );
}

export function PaneHead({ title, id, children }) {
  return (
    <div class="panel-header">
      <h2 id={id} class="cn-microlabel cn-nowrap">
        {title}
      </h2>
      <div class="cn-row cn-gap-4 cn-min-0">{children}</div>
    </div>
  );
}

// Prose with SQL in it: backticks in the source string mark the code spans, so
// the mono face lands on the keywords and not the sentence around them.
export function Marked({ text }) {
  return text
    .split("`")
    .map((part, i) => (i % 2 ? <code key={i}>{part}</code> : part));
}

export function Chip({ tone, title, class: extra = "", children }) {
  // Tint rides the package --tone contract prop via the cn-tone-* setters.
  const className = `${tone ? `chip is-toned cn-tone-${tone}` : "chip"} ${extra}`.trim();
  return (
    <span class={className} title={title}>
      {children}
    </span>
  );
}

export function Banner({ tone = "peach", children }) {
  return (
    <div class={`banner cn-tone-${tone}`}>
      {children}
    </div>
  );
}

export function EmptyState({ title, children }) {
  return (
    <div class="empty-state is-fill">
      <strong>{title}</strong>
      <span>{children}</span>
    </div>
  );
}

/** A flat button that reports back for a moment after copying. */
export function CopyButton({ text, label = "copy", icon }) {
  const [state, setState] = useState(null);
  useEffect(() => {
    if (!state) return undefined;
    const timer = setTimeout(() => setState(null), 1300);
    return () => clearTimeout(timer);
  }, [state]);

  return (
    <button
      class="btn btn-ghost is-sm"
      onClick={async () => {
        try {
          await navigator.clipboard.writeText(text);
          setState("copied");
        } catch {
          setState("copy failed");
        }
      }}
    >
      {state ?? (
        <>
          {icon ? <Icon id={icon} /> : null}
          {label}
        </>
      )}
    </button>
  );
}

/** A shell command line with the package copy control. */
export function CopyCommand({ command }) {
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    if (!copied) return undefined;
    const timer = setTimeout(() => setCopied(false), 1400);
    return () => clearTimeout(timer);
  }, [copied]);

  return (
    <div class={`command${copied ? " is-copied" : ""}`}>
      <code class="command-text">
        <span class="command-prompt">$</span>
        {command}
      </code>
      <button
        type="button"
        class="btn is-icon command-copy"
        aria-label="Copy to clipboard"
        title="Copy to clipboard"
        onClick={async () => {
          try {
            await navigator.clipboard.writeText(command);
            setCopied(true);
          } catch {
            /* clipboard denied; the command is still selectable */
          }
        }}
      >
        <svg class="copy-glyph" aria-hidden="true">
          <use href="#i-copy" />
        </svg>
        <svg class="done-glyph" aria-hidden="true">
          <use href="#i-check" />
        </svg>
      </button>
    </div>
  );
}

/**
 * A native <dialog> driven by a boolean. Esc and backdrop dismissal report back
 * through onClose so the caller's state stays authoritative.
 */
export function Modal({ open, onClose, className = "", children }) {
  const ref = useRef(null);
  const fromBackdrop = useRef(false);

  useEffect(() => {
    const dialog = ref.current;
    if (!dialog) return;
    if (open && !dialog.open) dialog.showModal();
    if (!open && dialog.open) dialog.close();
  }, [open]);

  // A click on the backdrop targets the dialog element itself. Requiring the
  // press to start there too keeps a drag that ends outside from dismissing.
  const press = (event) => {
    fromBackdrop.current = event.target === ref.current;
  };
  const release = (event) => {
    if (fromBackdrop.current && event.target === ref.current) onClose();
    fromBackdrop.current = false;
  };

  // Children stay mounted while closed: the dialog's own exit transition needs
  // something to fade out.
  return (
    <dialog
      ref={ref}
      class={className}
      onClose={onClose}
      onCancel={onClose}
      onMouseDown={press}
      onClick={release}
    >
      {children}
    </dialog>
  );
}
