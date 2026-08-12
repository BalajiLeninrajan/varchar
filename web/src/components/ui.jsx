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
    <section class={`pane ${className}`.trim()} {...rest}>
      {children}
    </section>
  );
}

export function PaneHead({ title, id, children }) {
  return (
    <div class="pane-head">
      <h2 id={id}>{title}</h2>
      <div class="head-chips">{children}</div>
    </div>
  );
}

export function Chip({ tone, title, children }) {
  const style = tone
    ? { color: `var(--${tone})`, borderColor: `color-mix(in srgb, var(--${tone}) 45%, transparent)` }
    : undefined;
  return (
    <span class="chip" style={style} title={title}>
      {children}
    </span>
  );
}

export function Banner({ tone = "peach", children }) {
  return (
    <div class="banner" style={{ "--tone": `var(--${tone})` }}>
      {children}
    </div>
  );
}

export function EmptyState({ title, children }) {
  return (
    <div class="empty-state">
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
      class="btn-flat"
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

/** A shell command that copies itself when clicked. */
export function CopyCommand({ command }) {
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    if (!copied) return undefined;
    const timer = setTimeout(() => setCopied(false), 1400);
    return () => clearTimeout(timer);
  }, [copied]);

  return (
    <button
      class={`install-copy${copied ? " is-copied" : ""}`}
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
      <span class="prompt">$</span>
      <code>{command}</code>
      <span class="copy-mark">{copied ? "copied" : <Icon id="copy" size={11} />}</span>
    </button>
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
