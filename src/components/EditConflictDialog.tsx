import type { EditChoice, EditConflict } from "../api";
import { formatSize, formatTime } from "../format";

interface Props {
  conflict: EditConflict;
  onAnswer: (choice: EditChoice) => void;
}

export function EditConflictDialog({ conflict, onAnswer }: Props) {
  return (
    <div className="modal-backdrop">
      <div className="modal" onKeyDown={(e) => e.key === "Escape" && onAnswer("keep")}>
        <h2>Il file è cambiato sul server</h2>
        <pre className="prompt">
          {conflict.host}:{conflict.remotePath}
        </pre>
        <p>Dopo che lo hai aperto, qualcun altro lo ha modificato. Caricando il tuo salvataggio perderesti quelle modifiche.</p>
        <table className="conflict-table">
          <tbody>
            <tr>
              <th>Quando l'hai aperto</th>
              <td>{formatSize(conflict.openedSize)}</td>
              <td>{formatTime(conflict.openedModified)}</td>
            </tr>
            <tr>
              <th>Ora sul server</th>
              <td>{formatSize(conflict.currentSize)}</td>
              <td>{formatTime(conflict.currentModified)}</td>
            </tr>
          </tbody>
        </table>
        <p className="muted small">
          "Usa versione del server" sostituisce la copia aperta nell'editor; la tua versione resta accanto come "(mia
          versione)".
        </p>
        <div className="actions">
          <button onClick={() => onAnswer("keep")}>Decido dopo</button>
          <button onClick={() => onAnswer("reload")}>Usa versione del server</button>
          <button className="primary danger" onClick={() => onAnswer("overwrite")}>
            Sovrascrivi sul server
          </button>
        </div>
      </div>
    </div>
  );
}
