import { useEffect, useState, type FormEvent } from "react";
import { settingsLoad, settingsSave, sshCheck, type Settings, type SshCheck } from "../api";

const DEFAULT_SSH = "/usr/bin/ssh";

interface Props {
  onClose: () => void;
}

/** "Impostazioni" (⌘,): settings stored by the backend. */
export function SettingsDialog({ onClose }: Props) {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [sshBin, setSshBin] = useState("");
  const [check, setCheck] = useState<{ path: string; result: SshCheck | string } | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    settingsLoad().then(
      (loaded) => {
        setSettings(loaded);
        setSshBin(loaded.sshBin ?? "");
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
    setError(null);
    settingsSave({ ...settings, sshBin: sshBin.trim() || null }).then(onClose, (err) => setError(String(err)));
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
