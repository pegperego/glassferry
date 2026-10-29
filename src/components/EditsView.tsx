import { useState } from "react";
import type { EditInfo } from "../api";
import { formatLogTime } from "../logStore";
import type { Edits } from "../useEdits";
import { ConflictDiffView } from "./ConflictDiffView";
import { ConfirmDialog } from "./Dialogs";

const STATE_LABEL: Record<EditInfo["state"], string> = {
  synced: "Sincronizzato",
  uploading: "Caricamento…",
  conflict: "Conflitto: modifiche locali non caricate",
  error: "Errore",
  unconfirmed: "Modifiche non caricate: caricamento non confermato",
  detached: "Non connesso: modifiche non caricate",
};

interface Props {
  edits: Edits;
  /** Show only this host's files. */
  host: string | null;
  hostColor: (host: string) => string | undefined;
  onReconnect: (connectionId: string) => void;
}

export function EditsView({ edits, host, hostColor, onReconnect }: Props) {
  const [closing, setClosing] = useState<EditInfo | null>(null);
  const [diffOf, setDiffOf] = useState<EditInfo | null>(null);
  const shown = host ? edits.edits.filter((e) => e.host === host) : edits.edits;
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
      {shown.map((edit) => (
        <div key={edit.id} className={`edit-row ${edit.state}`}>
          <span className={`edit-dot ${edit.state}`} title={STATE_LABEL[edit.state]} />
          <div className="transfer-main">
            <div className="transfer-title">
              <strong>{edit.remotePath.split("/").pop()}</strong>
              <span className="host-chip">
                <span className="category-dot" style={{ background: hostColor(edit.host) ?? "var(--muted)" }} />
                {edit.host}
              </span>
              <span className="muted" title={edit.remotePath}>
                {edit.remotePath}
              </span>
            </div>
            <div className="transfer-meta muted">
              <span>{STATE_LABEL[edit.state]}</span>
              <span>
                {edit.uploads === 0
                  ? "nessun salvataggio caricato"
                  : `${edit.uploads} salvataggi caricati, l'ultimo alle ${formatLogTime(edit.lastSyncAt).slice(0, 8)}`}
              </span>
              {edit.error && (
                <span className={edit.state === "synced" || edit.state === "detached" ? "" : "error-text"}>{edit.error}</span>
              )}
            </div>
          </div>
          {edit.state === "conflict" && (
            <>
              <button onClick={() => setDiffOf(edit)}>Differenze</button>
              <button onClick={() => edits.resolve(edit.id, "overwrite")}>Sovrascrivi sul server</button>
              <button onClick={() => edits.resolve(edit.id, "reload")}>Usa versione del server</button>
            </>
          )}
          {edit.state === "unconfirmed" && <button onClick={() => edits.upload(edit.id)}>Carica sul server</button>}
          {edit.state === "detached" && (
            <button title="Riapri la connessione: le modifiche vengono caricate" onClick={() => onReconnect(edit.connectionId)}>
              Riconnetti
            </button>
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
      {diffOf && (
        <div className="modal-backdrop">
          <div className="modal conflict-modal wide" onKeyDown={(e) => e.key === "Escape" && setDiffOf(null)}>
            <h2>Differenze con il server</h2>
            <pre className="prompt">
              {diffOf.host}:{diffOf.remotePath}
            </pre>
            <ConflictDiffView editId={diffOf.id} />
            <div className="actions">
              <button className="primary" autoFocus onClick={() => setDiffOf(null)}>
                Chiudi
              </button>
            </div>
          </div>
        </div>
      )}
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
