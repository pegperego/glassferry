import { useState, type FormEvent, type ReactNode } from "react";

interface PromptProps {
  title: string;
  label: string;
  initial: string;
  confirmLabel: string;
  /** Returns an error message, or null when the value is acceptable. */
  validate?: (value: string) => string | null;
  onSubmit: (value: string) => void;
  onCancel: () => void;
}

export function PromptDialog({ title, label, initial, confirmLabel, validate, onSubmit, onCancel }: PromptProps) {
  const [value, setValue] = useState(initial);
  const error = validate?.(value) ?? null;
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (!error) onSubmit(value);
  };
  return (
    <div className="modal-backdrop">
      <form className="modal" onSubmit={submit} onKeyDown={(e) => e.key === "Escape" && onCancel()}>
        <h2>{title}</h2>
        <label className="field">
          {label}
          <input autoFocus value={value} onChange={(e) => setValue(e.target.value)} onFocus={(e) => e.target.select()} />
        </label>
        {error && value !== "" && <p className="error-text small">{error}</p>}
        <div className="actions">
          <button type="button" onClick={onCancel}>
            Annulla
          </button>
          <button type="submit" className="primary" disabled={error !== null}>
            {confirmLabel}
          </button>
        </div>
      </form>
    </div>
  );
}

interface ConfirmProps {
  title: string;
  children: ReactNode;
  confirmLabel: string;
  danger?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}

export function ConfirmDialog({ title, children, confirmLabel, danger, onConfirm, onCancel }: ConfirmProps) {
  return (
    <div className="modal-backdrop">
      <div className="modal" onKeyDown={(e) => e.key === "Escape" && onCancel()}>
        <h2>{title}</h2>
        {children}
        <div className="actions">
          <button onClick={onCancel} autoFocus={danger}>
            Annulla
          </button>
          <button className={danger ? "primary danger" : "primary"} onClick={onConfirm} autoFocus={!danger}>
            {confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}

interface ChooseProps {
  title: string;
  children: ReactNode;
  options: { label: string; onSelect: () => void }[];
  onCancel: () => void;
}

/** A question with several answers (e.g. which pane to replace). */
export function ChooseDialog({ title, children, options, onCancel }: ChooseProps) {
  return (
    <div className="modal-backdrop">
      <div className="modal" onKeyDown={(e) => e.key === "Escape" && onCancel()}>
        <h2>{title}</h2>
        {children}
        <div className="actions">
          <button onClick={onCancel} autoFocus>
            Annulla
          </button>
          {options.map((o) => (
            <button key={o.label} className="primary" onClick={o.onSelect}>
              {o.label}
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}

/** Same rules as the backend (`fileops::validate_name`), for immediate feedback. */
export function nameError(name: string): string | null {
  if (name.trim() === "") return "Il nome non può essere vuoto";
  if (name === "." || name === "..") return "Nome non valido";
  if (name.includes("/")) return "Il nome non può contenere /";
  return null;
}
