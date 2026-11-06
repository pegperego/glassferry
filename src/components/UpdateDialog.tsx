import { updateOpenRelease } from "../api";
import { formatSize } from "../format";
import { percentOf, type RestartImpact } from "../updates";
import type { Updates } from "../useUpdates";

interface Props {
  updates: Updates;
  /** What restarting now would interrupt. */
  impact: RestartImpact;
  onClose: () => void;
}

const INSTALL_NOTE: Record<string, string> = {
  app: "Sostituisce l'app installata; la nuova versione parte al riavvio.",
  appimage: "Sostituisce il file AppImage; la nuova versione parte al riavvio.",
  deb: "Installa il pacchetto .deb: il sistema chiede la password di amministratore.",
  rpm: "Installa il pacchetto .rpm: il sistema chiede la password di amministratore.",
  nsis: "Avvia l'installazione: l'app si chiude e riparte con la nuova versione.",
  msi: "Avvia l'installazione: l'app si chiude e riparte con la nuova versione.",
};

/** The update found (notes, install, restart) or the result of a manual check. */
export function UpdateDialog({ updates, impact, onClose }: Props) {
  const { phase, info, progress, error } = updates;
  const close = () => {
    updates.dismiss();
    onClose();
  };

  if (phase === "checking" || phase === "idle" || phase === "upToDate" || (phase === "error" && !info)) {
    return (
      <div className="modal-backdrop">
        <div className="modal" onKeyDown={(e) => e.key === "Escape" && close()}>
          <h2>Aggiornamenti</h2>
          {phase === "checking" && <p className="muted">Controllo in corso…</p>}
          {phase === "upToDate" && <p>Glassferry è aggiornato: non ci sono versioni più recenti.</p>}
          {phase === "idle" && <p className="muted">Nessun aggiornamento da mostrare.</p>}
          {phase === "error" && <p className="error-text small">{error}</p>}
          <div className="actions">
            {phase === "error" && <button onClick={updates.check}>Riprova</button>}
            <button className="primary" autoFocus onClick={close}>
              Chiudi
            </button>
          </div>
        </div>
      </div>
    );
  }
  if (!info) return null;

  const percent = progress ? percentOf(progress.downloaded, progress.total) : null;
  const date = info.date ? new Date(info.date).toLocaleDateString("it-IT") : null;
  const interrupted = impact.transfers.length > 0;

  return (
    <div className="modal-backdrop">
      <div className="modal update" onKeyDown={(e) => e.key === "Escape" && phase !== "installing" && close()}>
        <h2>
          {phase === "installed" ? `Glassferry ${info.version} è installato` : `È disponibile Glassferry ${info.version}`}
        </h2>
        <p className="muted small">
          Versione in uso: {info.currentVersion}
          {date && ` · pubblicata il ${date}`}
        </p>
        {info.notes && <pre className="release-notes">{info.notes}</pre>}

        {phase === "available" && info.installer && <p className="small">{INSTALL_NOTE[info.installer]}</p>}
        {phase === "available" && !info.installer && (
          <p className="small">
            Questa copia di Glassferry non si aggiorna da sola (build di sviluppo o pacchetto non riconosciuto): scarica la
            nuova versione dalla pagina della release.
          </p>
        )}
        {phase === "installing" && (
          <>
            <div className="progress">
              <div className="progress-bar" style={{ width: `${percent ?? 0}%` }} />
            </div>
            <p className="muted small">
              {progress
                ? `Download: ${formatSize(progress.downloaded)}${progress.total ? ` di ${formatSize(progress.total)}` : ""}`
                : "Download in corso…"}
              {percent === 100 && " · installazione…"}
            </p>
          </>
        )}
        {phase === "installed" && (
          <>
            <p className="small">La nuova versione parte al riavvio: adesso, oppure la prossima volta che apri l'app.</p>
            {interrupted && (
              <p className="error-text small">
                Riavviando ora si interrompono {impact.transfers.length === 1 ? "un trasferimento" : `${impact.transfers.length} trasferimenti`} in
                corso: si potranno riprendere dalla scheda Trasferimenti.
              </p>
            )}
            {impact.edits.length > 0 && (
              <p className="muted small">
                {impact.edits.length === 1 ? "Un file aperto ha" : `${impact.edits.length} file aperti hanno`} modifiche non
                ancora caricate: non vanno perse, tornano in "File aperti" e vengono caricate alla prossima connessione.
              </p>
            )}
          </>
        )}
        {error && <p className="error-text small">{error}</p>}

        <div className="actions">
          {phase === "available" && (
            <>
              <button className="inline link about-link" onClick={() => { updates.skip(); onClose(); }}>
                Salta questa versione
              </button>
              <button onClick={close}>Più tardi</button>
              {info.installer ? (
                <button className="primary" autoFocus onClick={updates.install}>
                  {error ? "Riprova" : "Installa"}
                </button>
              ) : (
                <button className="primary" autoFocus onClick={() => void updateOpenRelease(info.version)}>
                  Apri la pagina della release
                </button>
              )}
            </>
          )}
          {phase === "installing" && <button disabled>Installazione…</button>}
          {phase === "installed" && (
            <>
              <button onClick={close} autoFocus={interrupted}>
                Al prossimo avvio
              </button>
              <button className={interrupted ? "primary danger" : "primary"} autoFocus={!interrupted} onClick={updates.restart}>
                Riavvia ora
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
