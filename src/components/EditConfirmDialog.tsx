import { useState } from "react";
import type { EditConfirm } from "../api";
import { confirmationMatches, type Protection } from "../connections";
import { formatSize } from "../format";
import { CommandPreview, NameGate, ProtectionBanner } from "./Protection";

interface Props {
  confirm: EditConfirm;
  protection: Protection | null;
  /** `remember`: do not ask again for this file. */
  onAnswer: (allow: boolean, remember: boolean) => void;
}

/** First upload of an edited file on a protected connection. */
export function EditConfirmDialog({ confirm, protection, onAnswer }: Props) {
  const [typed, setTyped] = useState("");
  const [remember, setRemember] = useState(false);
  const ready = confirmationMatches(protection, typed);
  return (
    <div className="modal-backdrop">
      <form
        className="modal protected-modal"
        style={protection?.category ? { borderColor: protection.category.color } : undefined}
        onKeyDown={(e) => e.key === "Escape" && onAnswer(false, false)}
        onSubmit={(e) => {
          e.preventDefault();
          if (ready) onAnswer(true, remember);
        }}
      >
        {protection && <ProtectionBanner protection={protection} />}
        <h2>Caricare il salvataggio sul server?</h2>
        <pre className="prompt">
          {confirm.remotePath} · {formatSize(confirm.size)}
        </pre>
        <p className="small">
          Hai salvato il file nell'editor. Sulle connessioni protette il primo caricamento di ogni file aperto va confermato;
          i salvataggi successivi vengono caricati da soli.
        </p>
        <CommandPreview lines={confirm.commands} open />
        <NameGate protection={protection} value={typed} onChange={setTyped} />
        <label className="checkbox">
          <input type="checkbox" checked={remember} onChange={(e) => setRemember(e.target.checked)} />
          Non chiedere più per questo file
        </label>
        <div className="actions">
          <button type="button" onClick={() => onAnswer(false, false)}>
            Non caricare
          </button>
          <button type="submit" className="primary danger" disabled={!ready}>
            Carica sul server
          </button>
        </div>
      </form>
    </div>
  );
}
