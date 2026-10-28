import { useState } from "react";
import type { ConflictAction, ConflictRequest } from "../api";
import { confirmationMatches, type Protection } from "../connections";
import { formatSize, formatTime } from "../format";
import { CommandPreview, NameGate, ProtectionBanner } from "./Protection";

interface Props {
  request: ConflictRequest;
  /** Destination connection of an upload; overwriting needs the typed name when protected. */
  protection: Protection | null;
  onAnswer: (action: ConflictAction, applyToAll: boolean) => void;
}

export function ConflictDialog({ request, protection, onAnswer }: Props) {
  const [applyToAll, setApplyToAll] = useState(false);
  const [typed, setTyped] = useState("");
  const canOverwrite = confirmationMatches(protection, typed);
  const where = request.direction === "download" ? "in locale" : `su ${request.host}`;
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
      <div
        className={protection?.protected ? "modal protected-modal" : "modal"}
        style={protection?.protected && protection.category ? { borderColor: protection.category.color } : undefined}
        onKeyDown={(e) => e.key === "Escape" && onAnswer("cancel", false)}
      >
        {protection && <ProtectionBanner protection={protection} />}
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
        <CommandPreview lines={request.commands} open={protection?.protected ?? false} />
        <NameGate protection={protection} value={typed} onChange={setTyped} />
        <label className="checkbox">
          <input type="checkbox" checked={applyToAll} onChange={(e) => setApplyToAll(e.target.checked)} />
          Applica a tutti i conflitti di questo trasferimento
        </label>
        <div className="actions">
          <button onClick={() => onAnswer("cancel", false)}>Annulla trasferimento</button>
          <button onClick={() => onAnswer("skip", applyToAll)}>Salta</button>
          <button
            className={protection?.protected ? "primary danger" : "primary"}
            autoFocus={!protection?.protected}
            disabled={!canOverwrite}
            onClick={() => onAnswer("overwrite", applyToAll)}
          >
            {applyToAll ? "Sovrascrivi tutti" : "Sovrascrivi"}
          </button>
        </div>
      </div>
    </div>
  );
}
