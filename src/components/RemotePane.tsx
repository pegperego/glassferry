import { useCallback, useEffect, useMemo, useState } from "react";
import { isDirLike, listDir, type ConnectResult, type RemoteEntry } from "../api";
import { formatPermissions, formatSize, formatTime } from "../format";
import { joinPath, parentPath, pathSegments } from "../paths";

interface Props {
  alias: string;
  connection: ConnectResult;
  onDisconnect: () => void;
}

export function RemotePane({ alias, connection, onDisconnect }: Props) {
  const [path, setPath] = useState(connection.home);
  const [entries, setEntries] = useState<RemoteEntry[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [showHidden, setShowHidden] = useState(false);
  const [selected, setSelected] = useState<string | null>(null);

  const load = useCallback(
    async (target: string) => {
      setLoading(true);
      setError(null);
      try {
        const result = await listDir(connection.sessionId, target);
        setEntries(result);
        setPath(target);
        setSelected(null);
      } catch (e) {
        setError(String(e));
      } finally {
        setLoading(false);
      }
    },
    [connection.sessionId],
  );

  useEffect(() => {
    void load(connection.home);
  }, [connection.home, load]);

  const visible = useMemo(
    () =>
      entries
        .filter((e) => showHidden || !e.name.startsWith("."))
        .sort((a, b) => Number(isDirLike(b)) - Number(isDirLike(a)) || a.name.localeCompare(b.name)),
    [entries, showHidden],
  );

  const open = (entry: RemoteEntry) => {
    if (isDirLike(entry)) void load(joinPath(path, entry.name));
  };

  return (
    <section className="pane">
      <header className="pane-header">
        <strong>{alias}</strong>
        <nav className="breadcrumb">
          {pathSegments(path).map((segment) => (
            <button key={segment.path} onClick={() => void load(segment.path)}>
              {segment.name}
            </button>
          ))}
        </nav>
        <label>
          <input type="checkbox" checked={showHidden} onChange={(e) => setShowHidden(e.target.checked)} />
          Nascosti
        </label>
        <button onClick={() => void load(path)} disabled={loading}>
          Aggiorna
        </button>
        <button onClick={onDisconnect}>Disconnetti</button>
      </header>
      <p className="capabilities">
        {connection.capabilities.shell
          ? `Shell ${connection.capabilities.os ?? ""}: ${connection.capabilities.commands.join(", ")}`
          : "Account solo SFTP"}
      </p>
      {error && <p className="error">{error}</p>}
      <div className="table-wrap">
        <table className="files">
          <thead>
            <tr>
              <th>Nome</th>
              <th className="num">Dimensione</th>
              <th>Modificato</th>
              <th>Permessi</th>
            </tr>
          </thead>
          <tbody>
            {path !== "/" && (
              <tr onDoubleClick={() => void load(parentPath(path))}>
                <td colSpan={4}>📁 ..</td>
              </tr>
            )}
            {visible.map((entry) => (
              <tr
                key={entry.name}
                className={entry.name === selected ? "selected" : undefined}
                onClick={() => setSelected(entry.name)}
                onDoubleClick={() => open(entry)}
              >
                <td>
                  {icon(entry)} {entry.name}
                </td>
                <td className="num">{isDirLike(entry) ? "" : formatSize(entry.size)}</td>
                <td>{formatTime(entry.modified)}</td>
                <td className="mono">{formatPermissions(entry.permissions)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </section>
  );
}

function icon(entry: RemoteEntry): string {
  if (isDirLike(entry)) return entry.kind === "symlink" ? "🔗" : "📁";
  return entry.kind === "symlink" ? "🔗" : "📄";
}
