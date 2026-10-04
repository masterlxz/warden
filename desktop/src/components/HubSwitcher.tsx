import type { SavedHub } from "../types";
import type { RemoteState } from "../lib/hub";

interface Props {
  hubs: SavedHub[];
  /** The hub in use; `null` is this computer. */
  activeHubId: string | null;
  state: RemoteState | undefined;
  busy: boolean;
  onPick: (hubId: string | null) => void;
  /** Signs in again to the hub in use (the token was turned away). */
  onSignIn: (hub: SavedHub) => void;
}

/** One line about the connection, for under the picker. */
export function describeState(state: RemoteState | undefined): { text: string; tone: "ok" | "wait" | "bad" } {
  if (!state || state.state === "connecting") return { text: "Connecting…", tone: "wait" };
  if (state.state === "retrying") return { text: `Can't reach the hub (${state.error}). Trying again in ${state.inSecs}s.`, tone: "wait" };
  if (state.state === "stopped") return { text: state.error ? `Disconnected: ${state.error}` : "Disconnected.", tone: "bad" };
  const user = state.user;
  if (!user) return { text: "Connected as the owner.", tone: "ok" };
  if (user.mustChangePassword) return { text: `Connected as ${user.name}, but the password is still the provisional one: change it on the hub's web page first.`, tone: "bad" };
  if (user.locked) return { text: `Connected as ${user.name}, but the data is locked (the hub restarted): disconnect and sign in with the password to unlock it.`, tone: "bad" };
  return { text: `Connected as ${user.name}.`, tone: "ok" };
}

/** Which machine's engine, conversations and projects the screens use: this computer, or a saved hub (P102). Switching
 * to a hub the first time asks how to sign in; afterwards the token the hub issued is enough. */
function HubSwitcher({ hubs, activeHubId, state, busy, onPick, onSignIn }: Props) {
  const active = hubs.find((h) => h.id === activeHubId);
  const status = activeHubId ? describeState(state) : null;
  return (
    <div className="hub-switcher">
      <label className="hub-switcher-label" htmlFor="hub-switcher-select">
        Machine
      </label>
      <select
        id="hub-switcher-select"
        className="hub-switcher-select"
        value={activeHubId ?? ""}
        disabled={busy}
        onChange={(e) => onPick(e.currentTarget.value || null)}
      >
        <option value="">This computer</option>
        {hubs.map((h) => (
          <option key={h.id} value={h.id}>
            {h.name}
          </option>
        ))}
      </select>
      {status && (
        <p className={`hub-switcher-status hub-switcher-status--${status.tone}`} role="status">
          {status.text}
          {active && state?.state === "stopped" && (
            <>
              {" "}
              <button type="button" className="hub-switcher-link" onClick={() => onSignIn(active)}>
                Sign in again
              </button>
            </>
          )}
        </p>
      )}
    </div>
  );
}

export default HubSwitcher;
