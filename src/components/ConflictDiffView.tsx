import { useEffect, useState } from "react";
import { editConflictDiff, type ConflictDiff } from "../api";

/** Lines of the server's version against the local copy of an edit in conflict. */
export function ConflictDiffView({ editId }: { editId: number }) {
  const [diff, setDiff] = useState<ConflictDiff | string | null>(null);

  useEffect(() => {
    editConflictDiff(editId).then(setDiff, (e) => setDiff(String(e)));
  }, [editId]);

  if (diff === null) return <p className="muted small">Confronto in corso…</p>;
  if (typeof diff === "string") return <p className="error-text small">{diff}</p>;
  if (diff.tooLarge) return <p className="muted small">File troppo grande per il confronto (oltre 2 MB).</p>;
  if (diff.binary) return <p className="muted small">File binario: non si confronta riga per riga.</p>;
  if (diff.identical) return <p className="small">Il contenuto è identico: è cambiata solo la data sul server.</p>;
  return (
    <div className="conflict-diff">
      <p className="small">
        <span className="diff-server">−{diff.serverOnly}</span> righe solo sul server (andrebbero perse sovrascrivendo) ·{" "}
        <span className="diff-mine">+{diff.mineOnly}</span> righe solo nella tua versione
        {diff.truncated && <span className="muted"> · confronto troncato</span>}
      </p>
      <div className="diff-lines mono">
        {diff.hunks.map((hunk, h) => (
          <div key={h} className="diff-hunk">
            {hunk.map((line, i) => (
              <div key={i} className={`diff-line ${line.kind}`}>
                <span className="diff-num">{line.serverLine ?? ""}</span>
                <span className="diff-num">{line.mineLine ?? ""}</span>
                <span className="diff-sign">{line.kind === "server" ? "−" : line.kind === "mine" ? "+" : " "}</span>
                <span className="diff-text">{line.text}</span>
              </div>
            ))}
          </div>
        ))}
      </div>
    </div>
  );
}
