import { useState } from "react";
import type { ConflictAction, ConflictRequest } from "../api";
import { formatSize, formatTime } from "../format";

interface Props {
  request: ConflictRequest;
  onAnswer: (action: ConflictAction, applyToAll: boolean) => void;
}

export function ConflictDialog({ request, onAnswer }: Props) {
  const [applyToAll, setApplyToAll] = useState(false);
  const where = request.direction === "upload" ? `su ${request.host}` : "in locale";
  const newer =
    request.srcModified !== null && request.dstModified !== null
      ? request.srcModified > request.dstModified
        ? "Il file che stai copiando è più recente."
        : request.srcModified < request.dstModified
          ? "Il file esistente è più recente."
          : "Stessa data di modifica."
      : null;

  return (
    <div className="modal-backdrop">
      <div className="modal" onKeyDown={(e) => e.key === "Escape" && onAnswer("cancel", false)}>
        <h2>Il file esiste già {where}</h2>
        <pre className="prompt">{request.path}</pre>
        <table className="conflict-table">
          <tbody>
            <tr>
              <th>Da copiare</th>
              <td>{formatSize(request.srcSize)}</td>
              <td>{formatTime(request.srcModified)}</td>
            </tr>
            <tr>
              <th>Esistente</th>
              <td>{formatSize(request.dstSize)}</td>
              <td>{formatTime(request.dstModified)}</td>
            </tr>
          </tbody>
        </table>
        {newer && <p className="muted">{newer}</p>}
        <label className="checkbox">
          <input type="checkbox" checked={applyToAll} onChange={(e) => setApplyToAll(e.target.checked)} />
          Applica a tutti i conflitti di questo trasferimento
        </label>
        <div className="actions">
          <button onClick={() => onAnswer("cancel", false)}>Annulla trasferimento</button>
          <button onClick={() => onAnswer("skip", applyToAll)}>Salta</button>
          <button className="primary" autoFocus onClick={() => onAnswer("overwrite", applyToAll)}>
            Sovrascrivi
          </button>
        </div>
      </div>
    </div>
  );
}
