import { createSignal, type Accessor } from "solid-js";

/**
 * A bump-and-show request from a global key to the panel that owns the
 * state. `App` bumps it; the panel's effect calls `take()` once its own
 * preconditions hold. The consumed mark lives here, with the nonce, not in
 * the panel: workspace panels remount on every tab switch, so a mark kept
 * in panel state reset to 0 and replayed the last request on remount. A
 * bump made while the panel was unmounted is still pending when it mounts.
 */
export interface Nonce {
  /** The bump count (tracked when read inside an effect). */
  readonly value: Accessor<number>;
  readonly bump: () => void;
  /** True exactly once per bump; marks it consumed. Tracks `value`. */
  readonly take: () => boolean;
}

export function createNonce(): Nonce {
  const [value, setValue] = createSignal(0);
  let handled = 0;
  return {
    value,
    bump: () => setValue((n) => n + 1),
    take: () => {
      const n = value();
      if (n === handled) return false;
      handled = n;
      return true;
    },
  };
}
