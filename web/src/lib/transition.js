import { useEffect, useState } from "preact/hooks";

/**
 * Keeps an element mounted for `duration` after it closes so it can play an
 * exit animation. Returns whether to render it, and whether it is leaving.
 */
export function useMountTransition(open, duration = 180) {
  const [mounted, setMounted] = useState(open);
  const [leaving, setLeaving] = useState(false);

  useEffect(() => {
    if (open) {
      setMounted(true);
      setLeaving(false);
      return undefined;
    }
    if (!mounted) return undefined;
    setLeaving(true);
    const timer = setTimeout(() => {
      setMounted(false);
      setLeaving(false);
    }, duration);
    return () => clearTimeout(timer);
  }, [open, mounted, duration]);

  return { mounted, leaving };
}
