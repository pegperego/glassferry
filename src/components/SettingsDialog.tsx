import { useEffect, useState, type FormEvent } from "react";
import {
  settingsLoad,
  settingsSave,
  sshCheck,
  terminalList,
  type Settings,
  type SshCheck,
  type TerminalApp,
  type TerminalSettings,
} from "../api";

const DEFAULT_SSH = "/usr/bin/ssh";
const IS_MAC = navigator.userAgent.includes("Mac");
const AUTO = "";
const CUSTOM = "custom";

/** Select value for the stored settings: a terminal id, AUTO or CUSTOM. */
function terminalChoice(terminal: TerminalSettings, installed: TerminalApp[]): string {
  if (terminal.command) return CUSTOM;
  if (!terminal.app) return AUTO;
  // Older settings stored the app name ("Terminal", "iTerm").
  const app = terminal.app.toLowerCase();
  return installed.find((t) => t.id === app || t.name.toLowerCase() === app)?.id ?? terminal.app;
}

function terminalSettings(choice: string, command: string): TerminalSettings {
  if (choice === CUSTOM) return { app: null, command: command.trim() || null };
  return { app: choice === AUTO ? null : choice, command: null };
}

interface Props {
  onClose: () => void;
  /** "Informazioni su Glassferry" (on Linux there is no app menu). */
  onAbout: () => void;
}

/** "Impostazioni" (⌘,): settings stored by the backend. */
export function SettingsDialog({ onClose, onAbout }: Props) {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [sshBin, setSshBin] = useState("");
  const [terminal, setTerminal] = useState(AUTO);
  const [installed, setInstalled] = useState<TerminalApp[]>([]);
  const [command, setCommand] = useState("");
  const [check, setCheck] = useState<{ path: string; result: SshCheck | string } | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    Promise.all([settingsLoad(), terminalList()]).then(
      ([loaded, terminals]) => {
        setInstalled(terminals);
        setSettings(loaded);
        setSshBin(loaded.sshBin ?? "");
        setTerminal(terminalChoice(loaded.terminal, terminals));
        setCommand(loaded.terminal.command ?? "");
      },
      (e) => setError(String(e)),
    );
  }, []);

  const verify = (path: string) => {
    setCheck(null);
    sshCheck(path || null).then(
      (result) => setCheck({ path, result }),
      (e) => setCheck({ path, result: String(e) }),
    );
  };

  const save = (e: FormEvent) => {
    e.preventDefault();
    if (!settings) return;
    if (terminal === CUSTOM && !command.trim()) {
      setError("Indica il comando del terminale, oppure scegline uno dall'elenco.");
      return;
    }
    setError(null);
    settingsSave({ ...settings, sshBin: sshBin.trim() || null, terminal: terminalSettings(terminal, command) }).then(
      onClose,
      (err) => setError(String(err)),
    );
  };

  return (
    <div className="modal-backdrop">
      <form className="modal settings" onSubmit={save} onKeyDown={(e) => e.key === "Escape" && onClose()}>
        <h2>Impostazioni</h2>
        {!settings && !error && <p className="muted">Caricamento…</p>}
        {settings && (
          <section>
            <h3>SSH</h3>
            <label className="field">
              Programma ssh
              <div className="field-row">
                <input
                  autoFocus
                  value={sshBin}
                  placeholder={DEFAULT_SSH}
                  spellCheck={false}
                  onChange={(e) => {
                    setSshBin(e.target.value);
                    setCheck(null);
                  }}
                />
                <button type="button" onClick={() => verify(sshBin.trim())}>
                  Verifica
                </button>
              </div>
            </label>
            {check && check.path === sshBin.trim() && <CheckResult result={check.result} />}
            <p className="muted small">
              Percorso assoluto; vuoto = {DEFAULT_SSH}. Per esempio l'ssh di Homebrew: /opt/homebrew/bin/ssh. Serve
              OpenSSH 8.4 o più recente. Vale per le connessioni aperte da ora in poi.
            </p>
          </section>
        )}
        {settings && (
          <section>
            <h3>Terminale</h3>
            <label className="field">
              "Apri nel terminale" (⌥⌘T) usa
              <select value={terminal} onChange={(e) => setTerminal(e.target.value)}>
                <option value={AUTO}>{IS_MAC ? "Predefinito (Terminal)" : "Predefinito ($TERMINAL, o il primo trovato)"}</option>
                {installed.map((t) => (
                  <option key={t.id} value={t.id}>
                    {t.name}
                  </option>
                ))}
                {terminal !== AUTO && terminal !== CUSTOM && !installed.some((t) => t.id === terminal) && (
                  <option value={terminal}>{terminal} (non trovato)</option>
                )}
                <option value={CUSTOM}>Comando personalizzato…</option>
              </select>
            </label>
            {terminal === CUSTOM && (
              <label className="field">
                Comando
                <input
                  value={command}
                  placeholder="mio-terminale -e {script}"
                  spellCheck={false}
                  onChange={(e) => setCommand(e.target.value)}
                />
              </label>
            )}
            <p className="muted small">
              Sul pannello locale il terminale si apre nella cartella corrente; su un pannello remoto apre una nuova
              connessione ssh allo stesso server e va nella cartella (l'eventuale password si inserisce nel terminale).
              {terminal === CUSTOM &&
                " Per un terminale non in elenco: il comando viene eseguito con sh e {script} è il percorso dello script da eseguire."}
            </p>
          </section>
        )}
        {error && <p className="error-text small">{error}</p>}
        <div className="actions">
          <button type="button" className="inline link about-link" onClick={onAbout}>
            Informazioni su Glassferry
          </button>
          <button type="button" onClick={onClose}>
            Annulla
          </button>
          <button type="submit" className="primary" disabled={!settings}>
            Salva
          </button>
        </div>
      </form>
    </div>
  );
}

function CheckResult({ result }: { result: SshCheck | string }) {
  if (typeof result === "string") return <p className="error-text small">{result}</p>;
  return (
    <p className={result.supported ? "small" : "error-text small"}>
      {result.version}
      {!result.supported && " — versione troppo vecchia o non OpenSSH: le richieste di password nell'app potrebbero non funzionare"}
    </p>
  );
}
