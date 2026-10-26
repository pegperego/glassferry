import { useMemo, useState } from "react";
import type { HostEntry } from "../api";

interface Props {
  hosts: HostEntry[];
  error: string | null;
  activeAlias: string | null;
  busy: boolean;
  /** Hosts in diagnostic mode (`ssh -v`). */
  verboseHosts: ReadonlySet<string>;
  onToggleVerbose: (alias: string) => void;
  onConnect: (alias: string) => void;
}

export function HostList({ hosts, error, activeAlias, busy, verboseHosts, onToggleVerbose, onConnect }: Props) {
  const [filter, setFilter] = useState("");
  const visible = useMemo(() => {
    const needle = filter.trim().toLowerCase();
    return needle ? hosts.filter((h) => h.alias.toLowerCase().includes(needle)) : hosts;
  }, [hosts, filter]);

  return (
    <aside className="hosts">
      <input
        className="hosts-filter"
        type="search"
        placeholder={`Cerca tra ${hosts.length} host…`}
        value={filter}
        onChange={(e) => setFilter(e.target.value)}
        autoFocus
      />
      {error && <p className="error">{error}</p>}
      <ul>
        {visible.map((host) => {
          const verbose = verboseHosts.has(host.alias);
          return (
            <li key={host.alias} className="host-item">
              <button
                className={host.alias === activeAlias ? "host active" : "host"}
                title={host.source}
                disabled={busy}
                onDoubleClick={() => onConnect(host.alias)}
                onKeyDown={(e) => e.key === "Enter" && onConnect(host.alias)}
              >
                {host.alias}
              </button>
              <button
                className={verbose ? "host-diag on" : "host-diag"}
                title={
                  verbose
                    ? "Modalità diagnostica attiva (ssh -v): output di debug nel log. Clic per disattivare."
                    : "Attiva la modalità diagnostica (ssh -v) alla prossima connessione"
                }
                onClick={() => onToggleVerbose(host.alias)}
              >
                🩺
              </button>
            </li>
          );
        })}
      </ul>
    </aside>
  );
}
