import { useMemo, useState } from "react";
import type { HostEntry } from "../api";

interface Props {
  hosts: HostEntry[];
  error: string | null;
  activeAlias: string | null;
  busy: boolean;
  onConnect: (alias: string) => void;
}

export function HostList({ hosts, error, activeAlias, busy, onConnect }: Props) {
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
        {visible.map((host) => (
          <li key={host.alias}>
            <button
              className={host.alias === activeAlias ? "host active" : "host"}
              title={host.source}
              disabled={busy}
              onDoubleClick={() => onConnect(host.alias)}
              onKeyDown={(e) => e.key === "Enter" && onConnect(host.alias)}
            >
              {host.alias}
            </button>
          </li>
        ))}
      </ul>
    </aside>
  );
}
