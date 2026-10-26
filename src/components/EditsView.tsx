import { useState } from "react";
import type { EditInfo } from "../api";
import { formatLogTime } from "../logStore";
import type { Edits } from "../useEdits";
import { ConfirmDialog } from "./Dialogs";

const STATE_LABEL: Record<EditInfo["state"], string> = {
  synced: "Sincronizzato",
  uploading: "Caricamento…",
  conflict: "Conflitto: modifiche locali non caricate",
  error: "Errore",
};

export function EditsView({ edits }: { edits: Edits }) {
  const [closing, setClosing] = useState<EditInfo | null>(null);
  if (edits.edits.length === 0) {
    return (
      <p className="transfers-empty muted">
        Nessun file aperto. Fai doppio clic su un file remoto (o usa "Modifica" dal menu) per aprirlo con l'app
        predefinita: ogni salvataggio viene caricato sul server.
      </p>
    );
  }
  return (
    <div className="transfers">
      {edits.edits.map((edit) => (
        <div key={edit.id} className={`edit-row ${edit.state}`}>
          <span className={`edit-dot ${edit.state}`} title={STATE_LABEL[edit.state]} />
          <div className="transfer-main">
            <div className="transfer-title">
              <strong>{edit.remotePath.split("/").pop()}</strong>
              <span className="muted" title={edit.remotePath}>
                {edit.host}:{edit.remotePath}
              </span>
            </div>
            <div className="transfer-meta muted">
              <span>{STATE_LABEL[edit.state]}</span>
              <span>
                {edit.uploads === 0
                  ? "nessun salvataggio caricato"
                  : `${edit.uploads} salvataggi caricati, l'ultimo alle ${formatLogTime(edit.lastSyncAt).slice(0, 8)}`}
              </span>
              {edit.error && <span className={edit.state === "synced" ? "" : "error-text"}>{edit.error}</span>}
            </div>
          </div>
          {edit.state === "conflict" && (
            <>
              <button onClick={() => edits.resolve(edit.id, "overwrite")}>Sovrascrivi sul server</button>
              <button onClick={() => edits.resolve(edit.id, "reload")}>Usa versione del server</button>
            </>
          )}
          <button onClick={() => edits.reopen(edit.id)}>Apri</button>
          <button title="Mostra la copia locale nel Finder" onClick={() => edits.reveal(edit.id)}>
            Mostra
          </button>
          <button
            title={edit.state === "synced" ? "Chiudi e rimuovi la copia locale" : "Ci sono modifiche non caricate"}
            className={edit.state === "synced" ? undefined : "danger-outline"}
            onClick={() => (edit.state === "synced" ? edits.close(edit.id) : setClosing(edit))}
          >
            Chiudi
          </button>
        </div>
      ))}
      {closing && (
        <ConfirmDialog
          title="Chiudere senza caricare?"
          confirmLabel="Chiudi e perdi le modifiche"
          danger
          onCancel={() => setClosing(null)}
          onConfirm={() => {
            edits.close(closing.id);
            setClosing(null);
          }}
        >
          <p>
            La copia locale di <strong>{closing.remotePath}</strong> ha modifiche che non sono sul server. Chiudendo, la
            copia viene eliminata.
          </p>
        </ConfirmDialog>
      )}
    </div>
  );
}
