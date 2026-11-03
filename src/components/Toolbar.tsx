export type RemoteLayout = "side" | "stacked";

interface Props {
  /** Names of the remote panes in use (with … while connecting). */
  status: string[];
  onQuickConnect: () => void;
  onManage: () => void;
  layout: RemoteLayout;
  onLayout: (layout: RemoteLayout) => void;
  panelOpen: boolean;
  onTogglePanel: () => void;
  onSettings: () => void;
  /** A newer version found (or installed, waiting for a restart): opens its dialog. */
  update: { version: string; installed: boolean } | null;
  onUpdate: () => void;
}

export function Toolbar({ status, onQuickConnect, onManage, layout, onLayout, panelOpen, onTogglePanel, onSettings, update, onUpdate }: Props) {
  return (
    <header className="toolbar">
      <button className="primary" title="Nuovo collegamento (⌘K)" onClick={onQuickConnect}>
        + Nuovo collegamento <kbd>⌘K</kbd>
      </button>
      <button onClick={onManage}>Gestisci connessioni</button>
      <div className="segmented" role="group" aria-label="Disposizione dei pannelli remoti">
        <button
          className={layout === "side" ? "toggle on" : ""}
          title="Pannelli remoti affiancati"
          onClick={() => onLayout("side")}
        >
          ▥ Affiancati
        </button>
        <button
          className={layout === "stacked" ? "toggle on" : ""}
          title="Pannelli remoti impilati"
          onClick={() => onLayout("stacked")}
        >
          ▤ Impilati
        </button>
      </div>
      <span className="spacer" />
      {status.length > 0 && <span className="muted small">{status.join(" · ")}</span>}
      {update && (
        <button
          className="update-badge"
          title={update.installed ? `Riavvia per usare la versione ${update.version}` : `Disponibile la versione ${update.version}`}
          onClick={onUpdate}
        >
          {update.installed ? `↻ Riavvia per la ${update.version}` : `↑ Aggiornamento ${update.version}`}
        </button>
      )}
      <button className={panelOpen ? "toggle on" : ""} title="Pannello in basso (⌘J)" onClick={onTogglePanel}>
        Pannello <kbd>⌘J</kbd>
      </button>
      <button title="Impostazioni (⌘,)" aria-label="Impostazioni" onClick={onSettings}>
        ⚙
      </button>
    </header>
  );
}
