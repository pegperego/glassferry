import { useEffect, useState, type FormEvent } from "react";
import { settingsLoad, settingsSave, sshCheck, type Settings, type SshCheck, type TerminalSettings } from "../api";

const DEFAULT_SSH = "/usr/bin/ssh";
const IS_MAC = navigator.userAgent.includes("Mac");
/** macOS apps that run a `.command` script with `open -a`. */
const MAC_APPS = ["Terminal", "iTerm"];
const CUSTOM = "custom";
const LINUX_DEFAULT = "default";

/** Select value for the stored terminal settings. */
function terminalChoice(terminal: TerminalSettings): string {
  if (terminal.command) return CUSTOM;
  return IS_MAC ? (terminal.app ?? MAC_APPS[0]) : LINUX_DEFAULT;
}

function terminalSettings(choice: string, command: string): TerminalSettings {
  if (choice === CUSTOM) return { app: null, command: command.trim() || null };
  if (!IS_MAC || choice === MAC_APPS[0]) return { app: null, command: null };
  return { app: choice, command: null };
}

interface Props {
  onClose: () => void;
}

/** "Impostazioni" (⌘,): settings stored by the backend. */
export function SettingsDialog({ onClose }: Props) {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [sshBin, setSshBin] = useState("");
  const [terminal, setTerminal] = useState(LINUX_DEFAULT);
  const [command, setCommand] = useState("");
  const [check, setCheck] = useState<{ path: string; result: SshCheck | string } | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    settingsLoad().then(
      (loaded) => {
        setSettings(loaded);
        setSshBin(loaded.sshBin ?? "");
        setTerminal(terminalChoice(loaded.terminal));
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
                {IS_MAC ? (
                  [...new Set([...MAC_APPS, terminalChoice(settings.terminal)])]
                    .filter((app) => app !== CUSTOM)
                    .map((app) => (
                      <option key={app} value={app}>
                        {app}
                      </option>
                    ))
                ) : (
                  <option value={LINUX_DEFAULT}>Predefinito ($TERMINAL o x-terminal-emulator)</option>
                )}
                <option value={CUSTOM}>Comando personalizzato…</option>
              </select>
            </label>
            {terminal === CUSTOM && (
              <label className="field">
                Comando
                <input
                  value={command}
                  placeholder={IS_MAC ? "open -na Ghostty --args -e {script}" : "kitty {script}"}
                  spellCheck={false}
                  onChange={(e) => setCommand(e.target.value)}
                />
              </label>
            )}
            <p className="muted small">
              Sul pannello locale il terminale si apre nella cartella corrente; su un pannello remoto apre una nuova
              connessione ssh allo stesso server e va nella cartella (l'eventuale password si inserisce nel terminale).
              {terminal === CUSTOM &&
                " Il comando viene eseguito con sh; {script} è il percorso dello script da eseguire. Esempi: kitty {script} · wezterm start -- {script} · alacritty -e {script}."}
            </p>
          </section>
        )}
        {error && <p className="error-text small">{error}</p>}
        <div className="actions">
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
