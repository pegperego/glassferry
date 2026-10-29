import { useEffect, useRef, useState, type FormEvent } from "react";
import type { SearchKind, SearchQuery, SearchResult } from "../api";

const METHOD: Record<SearchResult["method"], string> = {
  find: "con find sul server",
  sftp: "visitando le cartelle via SFTP (account senza shell)",
  local: "visitando le cartelle",
};

interface Props {
  /** Folder searched, with its subfolders. */
  dir: string;
  search: (dir: string, query: SearchQuery, id: number) => Promise<SearchResult>;
  cancel: (id: number) => void;
  /** A result was chosen: show it in the pane. */
  onOpen: (path: string, dir: boolean) => void;
  onClose: () => void;
}

let nextSearchId = 1;

/** ⌘F: files and folders below the pane's folder, by name. */
export function SearchDialog({ dir, search, cancel, onOpen, onClose }: Props) {
  const [text, setText] = useState("");
  const [kind, setKind] = useState<SearchKind>("any");
  const [running, setRunning] = useState<number | null>(null);
  const [result, setResult] = useState<SearchResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const runningRef = useRef<number | null>(null);
  runningRef.current = running;

  // Closing the dialog stops a search still running.
  useEffect(() => () => {
    if (runningRef.current !== null) cancel(runningRef.current);
  }, [cancel]);

  const start = (e: FormEvent) => {
    e.preventDefault();
    if (running !== null) cancel(running);
    const id = nextSearchId++;
    setRunning(id);
    setError(null);
    setResult(null);
    search(dir, { text, kind }, id).then(
      (found) => {
        if (runningRef.current !== id) return;
        setResult(found);
        setRunning(null);
      },
      (err) => {
        if (runningRef.current !== id) return;
        const message = String(err);
        setError(message.includes("annullat") ? null : message);
        setRunning(null);
      },
    );
  };

  const stop = () => {
    if (running !== null) cancel(running);
    setRunning(null);
  };

  const relative = (path: string) => path.slice(dir.replace(/\/$/, "").length + 1);

  return (
    <div className="modal-backdrop top">
      <form className="modal search-dialog" onSubmit={start} onKeyDown={(e) => e.key === "Escape" && onClose()}>
        <h2>Cerca in {dir}</h2>
        <div className="field-row">
          <input
            autoFocus
            value={text}
            placeholder="Nome, o un modello come *.conf"
            spellCheck={false}
            onChange={(e) => setText(e.target.value)}
          />
          <select value={kind} onChange={(e) => setKind(e.target.value as SearchKind)} aria-label="Tipo">
            <option value="any">File e cartelle</option>
            <option value="file">Solo file</option>
            <option value="dir">Solo cartelle</option>
          </select>
          {/* Both always there: swapping them would turn the click on Interrompi into a new search. */}
          <button type="submit" className="primary" disabled={!text.trim() || running !== null}>
            Cerca
          </button>
          <button type="button" onClick={stop} disabled={running === null}>
            Interrompi
          </button>
        </div>
        <p className="muted small">Anche nelle sottocartelle, senza distinguere maiuscole; non entra in altri dischi o mount.</p>
        {running !== null && <p className="muted small">Ricerca in corso…</p>}
        {error && <p className="error-text small">{error}</p>}
        {result && (
          <>
            <p className="small">
              {result.matches.length === 0
                ? "Nessun risultato"
                : `${result.matches.length}${result.truncated ? "+" : ""} risultati`}
              <span className="muted">
                {" "}
                · {METHOD[result.method]}
                {result.truncated && " · elenco limitato: restringi la ricerca"}
                {result.partial && " · alcune cartelle non erano leggibili"}
              </span>
            </p>
            {result.matches.length > 0 && (
              <ul className="search-results">
                {result.matches.map((m) => (
                  <li key={m.path}>
                    <button type="button" className="search-result mono" title={m.path} onClick={() => onOpen(m.path, m.dir)}>
                      {m.dir ? "📁 " : ""}
                      {relative(m.path)}
                      {m.dir ? "/" : ""}
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </>
        )}
        <div className="actions">
          <button type="button" onClick={onClose}>
            Chiudi
          </button>
        </div>
      </form>
    </div>
  );
}
