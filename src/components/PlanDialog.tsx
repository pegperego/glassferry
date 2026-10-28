import { useState, type ReactNode } from "react";
import type { PlanPreview } from "../api";
import { confirmationMatches, type Protection } from "../connections";
import { CommandPreview, NameGate, ProtectionBanner } from "./Protection";

interface Props {
  title: string;
  /** null while the plan is being prepared. */
  preview: PlanPreview | null;
  error: string | null;
  /** Remote connection of the operation; null for the local pane. */
  protection: Protection | null;
  confirmLabel: string;
  danger?: boolean;
  children?: ReactNode;
  onConfirm: () => void;
  onCancel: () => void;
}

/**
 * Confirmation of a planned operation: what will run, and on protected
 * connections the connection name typed by the user.
 */
export function PlanDialog({ title, preview, error, protection, confirmLabel, danger, children, onConfirm, onCancel }: Props) {
  const [typed, setTyped] = useState("");
  const ready = preview !== null && confirmationMatches(protection, typed);
  return (
    <div className="modal-backdrop">
      <form
        className={protection?.protected ? "modal protected-modal" : "modal"}
        style={protection?.protected && protection.category ? { borderColor: protection.category.color } : undefined}
        onKeyDown={(e) => e.key === "Escape" && onCancel()}
        onSubmit={(e) => {
          e.preventDefault();
          if (ready) onConfirm();
        }}
      >
        {protection && <ProtectionBanner protection={protection} />}
        <h2>{title}</h2>
        {children}
        {error ? (
          <p className="error-text small">{error}</p>
        ) : preview === null ? (
          <p className="muted small">Preparazione dell'anteprima…</p>
        ) : (
          <CommandPreview
            explanation={preview.explanation}
            lines={preview.lines}
            more={preview.more}
            open={protection?.protected ?? false}
          />
        )}
        <NameGate protection={protection} value={typed} onChange={setTyped} />
        <div className="actions">
          <button type="button" onClick={onCancel} autoFocus={!protection?.protected}>
            Annulla
          </button>
          <button type="submit" className={danger ? "primary danger" : "primary"} disabled={!ready || error !== null}>
            {confirmLabel}
          </button>
        </div>
      </form>
    </div>
  );
}
