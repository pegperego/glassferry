import type { Protection } from "../connections";

/** Connection name and category at the top of a confirmation. */
export function ProtectionBanner({ protection }: { protection: Protection }) {
  const color = protection.category?.color ?? "var(--muted)";
  return (
    <div className={protection.protected ? "protection-banner protected" : "protection-banner"} style={{ borderColor: color }}>
      <strong>{protection.name}</strong>
      {protection.category && (
        <span className="category-badge" style={{ background: color }}>
          {protection.category.name}
        </span>
      )}
      {protection.protected && <span className="small">connessione protetta</span>}
    </div>
  );
}

/** "Type the connection name to confirm", shown only on protected connections. */
export function NameGate({
  protection,
  value,
  onChange,
}: {
  protection: Protection | null | undefined;
  value: string;
  onChange: (value: string) => void;
}) {
  if (!protection?.protected) return null;
  return (
    <label className="field name-gate">
      <span>
        Per confermare scrivi <strong className="mono">{protection.name}</strong>
      </span>
      <input
        autoFocus
        spellCheck={false}
        autoComplete="off"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        aria-label="Nome della connessione"
      />
    </label>
  );
}

/** Commands that will run; collapsed unless `open`. */
export function CommandPreview({
  explanation,
  lines,
  more,
  open,
}: {
  explanation?: string;
  lines: string[];
  more?: number;
  open: boolean;
}) {
  const body = (
    <>
      {explanation && <p className="small">{explanation}</p>}
      <pre className="command-preview">
        {lines.join("\n")}
        {more ? `\n… altre ${more} righe (l'elenco completo è nel log)` : ""}
      </pre>
    </>
  );
  return (
    <details className="command-details" open={open}>
      <summary className="muted small">Comandi che verranno eseguiti</summary>
      {body}
    </details>
  );
}
