interface Props {
  /** Name of the open connection, or the one being opened. */
  current: string | null;
  onQuickConnect: () => void;
  onManage: () => void;
  panelOpen: boolean;
  onTogglePanel: () => void;
}

export function Toolbar({ current, onQuickConnect, onManage, panelOpen, onTogglePanel }: Props) {
  return (
    <header className="toolbar">
      <button className="primary" title="Nuovo collegamento (⌘K)" onClick={onQuickConnect}>
        + Nuovo collegamento <kbd>⌘K</kbd>
      </button>
      <button onClick={onManage}>Gestisci connessioni</button>
      <span className="spacer" />
      {current && <span className="muted small">{current}</span>}
      <button className={panelOpen ? "toggle on" : ""} title="Pannello in basso (⌘J)" onClick={onTogglePanel}>
        Pannello <kbd>⌘J</kbd>
      </button>
    </header>
  );
}
