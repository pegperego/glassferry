import { useEffect, useState, type FormEvent } from "react";
import type { AskpassRequest } from "../api";

interface Props {
  request: AskpassRequest;
  onAnswer: (value: string | null) => void;
}

/** One ssh prompt: secret input, yes/no, host key confirmation or notice. */
export function AskpassDialog({ request, onAnswer }: Props) {
  const [value, setValue] = useState("");
  useEffect(() => setValue(""), [request.id]);

  const submit = (e: FormEvent) => {
    e.preventDefault();
    onAnswer(value);
  };

  return (
    <div className="modal-backdrop">
      <form className="modal" onSubmit={submit} onKeyDown={(e) => e.key === "Escape" && onAnswer(null)}>
        <h2>{title(request)}</h2>
        <pre className="prompt">{request.prompt.trim()}</pre>

        {request.kind === "secret" && (
          <input type="password" autoFocus value={value} onChange={(e) => setValue(e.target.value)} />
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
