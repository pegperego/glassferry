import { useEffect, useState } from "react";
import type { AttrChange, FileEntry } from "../api";
import { confirmationMatches, type Protection } from "../connections";
import { formatPermissions } from "../format";
import { CommandPreview, NameGate, ProtectionBanner } from "./Protection";

const CLASSES = [
  { label: "Proprietario", shift: 6 },
  { label: "Gruppo", shift: 3 },
  { label: "Altri", shift: 0 },
];
const BITS = [
  { label: "Lettura", bit: 4 },
  { label: "Scrittura", bit: 2 },
  { label: "Esecuzione", bit: 1 },
];

interface Props {
  dir: string;
  entries: FileEntry[];
  /** Preview (`dryRun`) or apply; resolves to the commands. */
  attrs: (dir: string, names: string[], change: AttrChange, dryRun: boolean) => Promise<string[]>;
  /** Owner and group can change (remote account with chown/chgrp). */
  canChown: boolean;
  protection: Protection | null;
  onDone: () => void;
  onCancel: () => void;
}

/** chmod / chown of a selection, with the commands that will run. */
export function PermissionsDialog({ dir, entries, attrs, canChown, protection, onDone, onCancel }: Props) {
  const names = entries.map((e) => e.name);
  const initial = (entries[0]?.permissions ?? 0o644) & 0o7777;
  const [changeMode, setChangeMode] = useState(true);
  const [mode, setMode] = useState(initial);
  const [octal, setOctal] = useState(initial.toString(8).padStart(3, "0"));
  const [recursive, setRecursive] = useState(false);
  const [owner, setOwner] = useState("");
  const [group, setGroup] = useState("");
  const [preview, setPreview] = useState<{ lines: string[] } | { error: string } | null>(null);
  const [typed, setTyped] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const hasDirs = entries.some((e) => e.kind === "dir");

  const change: AttrChange = {
    mode: changeMode ? mode : null,
    owner: owner.trim() || null,
    group: group.trim() || null,
    recursive: hasDirs && recursive,
  };
  const key = JSON.stringify(change);

  useEffect(() => {
    let current = true;
    setPreview(null);
    attrs(dir, names, change, true).then(
      (lines) => current && setPreview({ lines }),
      (e) => current && setPreview({ error: String(e) }),
    );
    return () => {
      current = false;
    };
    // `key` covers `change`; names and dir do not change while open.
  }, [key]);

  const setBits = (next: number) => {
    setMode(next);
    setOctal(next.toString(8).padStart(3, "0"));
  };
  const ready = preview !== null && "lines" in preview && !busy && confirmationMatches(protection, typed);

  const apply = () => {
    setBusy(true);
    setError(null);
    attrs(dir, names, change, false).then(onDone, (e) => {
      setBusy(false);
      setError(String(e));
    });
  };

  const what = entries.length === 1 ? entries[0].name : `${entries.length} elementi`;
  return (
    <div className="modal-backdrop">
      <form
        className={protection?.protected ? "modal permissions protected-modal" : "modal permissions"}
        style={protection?.protected && protection.category ? { borderColor: protection.category.color } : undefined}
        onKeyDown={(e) => e.key === "Escape" && onCancel()}
        onSubmit={(e) => {
          e.preventDefault();
          if (ready) apply();
        }}
      >
        {protection && <ProtectionBanner protection={protection} />}
        <h2>Permessi di {what}</h2>
        <label className="check">
          <input type="checkbox" checked={changeMode} onChange={(e) => setChangeMode(e.target.checked)} />
          Cambia i permessi
        </label>
        {changeMode && (
          <div className="perm-editor">
            <table className="perm-grid">
              <thead>
                <tr>
                  <th />
                  {BITS.map((b) => (
                    <th key={b.bit}>{b.label}</th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {CLASSES.map((c) => (
                  <tr key={c.shift}>
                    <th>{c.label}</th>
                    {BITS.map((b) => {
                      const bit = b.bit << c.shift;
                      return (
                        <td key={b.bit}>
                          <input
                            type="checkbox"
                            aria-label={`${c.label}: ${b.label}`}
                            checked={(mode & bit) !== 0}
                            onChange={(e) => setBits(e.target.checked ? mode | bit : mode & ~bit)}
                          />
                        </td>
                      );
                    })}
                  </tr>
                ))}
              </tbody>
            </table>
            <label className="field">
              Ottale
              <input
                className="mono"
                value={octal}
                maxLength={4}
                spellCheck={false}
                onChange={(e) => {
                  setOctal(e.target.value);
                  if (/^[0-7]{3,4}$/.test(e.target.value)) setMode(parseInt(e.target.value, 8));
                }}
              />
              <span className="mono">{formatPermissions(mode)}</span>
            </label>
          </div>
        )}
        {canChown && (
          <div className="field-row">
            <label className="field">
              Proprietario
              <input value={owner} placeholder="invariato" spellCheck={false} onChange={(e) => setOwner(e.target.value)} />
            </label>
            <label className="field">
              Gruppo
              <input value={group} placeholder="invariato" spellCheck={false} onChange={(e) => setGroup(e.target.value)} />
            </label>
          </div>
        )}
        {hasDirs && (
          <label className="check">
            <input type="checkbox" checked={recursive} onChange={(e) => setRecursive(e.target.checked)} />
            Anche il contenuto delle cartelle (stessi permessi per file e cartelle; i link non vengono seguiti)
          </label>
        )}
        {preview && "error" in preview && <p className="error-text small">{preview.error}</p>}
        {preview && "lines" in preview && <CommandPreview lines={preview.lines} open={protection?.protected ?? false} />}
        <NameGate protection={protection} value={typed} onChange={setTyped} />
        {error && <p className="error-text small">{error}</p>}
        <div className="actions">
          <button type="button" onClick={onCancel}>
            Annulla
          </button>
          <button type="submit" className="primary" disabled={!ready}>
            Applica
          </button>
        </div>
      </form>
    </div>
  );
}
