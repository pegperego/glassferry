import { useEffect, useState, type FormEvent } from "react";
import type { AskpassRequest } from "../api";

interface Props {
  request: AskpassRequest;
  /** `save`: keep the password in the keychain once connected. */
  onAnswer: (value: string | null, save?: boolean) => void;
}

/** One ssh prompt: secret input, yes/no, host key confirmation or notice. */
export function AskpassDialog({ request, onAnswer }: Props) {
  const [value, setValue] = useState("");
  const [save, setSave] = useState(true);
  useEffect(() => {
    setValue("");
    setSave(true);
  }, [request.id]);

  const submit = (e: FormEvent) => {
    e.preventDefault();
    onAnswer(value, request.canSave && save);
  };

  return (
    <div className="modal-backdrop">
      <form className="modal" onSubmit={submit} onKeyDown={(e) => e.key === "Escape" && onAnswer(null)}>
        <h2>{title(request)}</h2>
        {request.savedInvalid && (
          <p className="error-text small">La password salvata nel Portachiavi non è stata accettata.</p>
        )}
        <pre className="prompt">{request.prompt.trim()}</pre>

        {request.kind === "secret" && (
          <input type="password" autoFocus value={value} onChange={(e) => setValue(e.target.value)} />
        )}
        {request.kind === "secret" && request.canSave && (
          <label className="checkbox">
            <input type="checkbox" checked={save} onChange={(e) => setSave(e.target.checked)} />
            {request.savedInvalid ? "Aggiorna la password nel Portachiavi" : "Salva nel Portachiavi"}
          </label>
        )}

        <div className="actions">
          {request.kind === "notice" ? (
            <button type="button" onClick={() => onAnswer(null)} autoFocus>
              OK
            </button>
          ) : request.kind === "hostKey" ? (
            <>
              <button type="button" onClick={() => onAnswer("no")}>
                Rifiuta
              </button>
              <button type="button" className="primary" onClick={() => onAnswer("yes")}>
                Accetta e ricorda
              </button>
            </>
          ) : (
            <>
              <button type="button" onClick={() => onAnswer(null)}>
                Annulla
              </button>
              <button type="submit" className="primary" autoFocus={request.kind === "confirm"}>
                {request.kind === "confirm" ? "Conferma" : "OK"}
              </button>
            </>
          )}
        </div>
      </form>
    </div>
  );
}

function title(request: AskpassRequest): string {
  switch (request.kind) {
    case "hostKey":
      return `Host sconosciuto: ${request.alias}`;
    case "confirm":
      return `Conferma richiesta da ${request.alias}`;
    case "notice":
      return request.alias;
    case "secret":
      return `Autenticazione su ${request.alias}`;
  }
}
