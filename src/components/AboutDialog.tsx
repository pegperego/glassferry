import { useEffect, useMemo, useState } from "react";
import { appInfo, openRepository, sshCheck, type AppInfo } from "../api";
import icon from "../../src-tauri/icons/128x128@2x.png";

/** `src/generated/third-party.json`, written by scripts/third-party.mjs. */
interface ThirdParty {
  packages: { name: string; version: string; license: string; source: string; url: string; texts: number[] }[];
  /** License texts, referenced by index. */
  texts: string[];
}

const OS_NAME: Record<string, string> = { macos: "macOS", linux: "Linux" };

interface Props {
  onClose: () => void;
}

/** "Informazioni su Glassferry": version, license and third-party components. */
export function AboutDialog({ onClose }: Props) {
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [ssh, setSsh] = useState<string | null>(null);
  const [section, setSection] = useState<"license" | "thirdParty" | null>(null);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    void appInfo().then(setInfo);
    sshCheck(null).then(
      (check) => setSsh(check.version),
      (e) => setSsh(String(e)),
    );
  }, []);

  if (!info) return null;
  const build = info.commit ? `commit ${info.commit}${info.commitDate ? ` del ${info.commitDate}` : ""}` : null;
  const system = `${OS_NAME[info.os] ?? info.os} ${info.arch}`;
  const summary = [`${info.name} ${info.version}${build ? ` (${build})` : ""}`, system, ssh ?? ""].filter(Boolean).join("\n");

  return (
    <div className="modal-backdrop">
      <div className="modal about" onKeyDown={(e) => e.key === "Escape" && onClose()}>
        <div className="about-head">
          <img src={icon} alt="" width={72} height={72} />
          <div>
            <h2>{info.name}</h2>
            <p className="about-version">Versione {info.version}</p>
            {build && <p className="muted small">{build}</p>}
          </div>
        </div>
        <p className="small">Client SFTP che usa l'OpenSSH del sistema e il tuo ~/.ssh/config.</p>
        <dl className="about-facts small">
          <dt>Sistema</dt>
          <dd>{system}</dd>
          <dt>ssh</dt>
          <dd className="mono">{ssh ?? "…"}</dd>
          <dt>Licenza</dt>
          <dd>{info.license}, software libero e open source</dd>
          <dt>Codice sorgente</dt>
          <dd>
            <button className="inline link" onClick={() => void openRepository()}>
              {info.repository.replace(/^https:\/\//, "")}
            </button>
          </dd>
        </dl>
        <div className="segmented about-tabs" role="group">
          <button className={section === "license" ? "toggle on" : ""} onClick={() => setSection(section === "license" ? null : "license")}>
            Licenza
          </button>
          <button
            className={section === "thirdParty" ? "toggle on" : ""}
            onClick={() => setSection(section === "thirdParty" ? null : "thirdParty")}
          >
            Componenti di terze parti
          </button>
        </div>
        {section === "license" && <pre className="license-text">{info.licenseText}</pre>}
        {section === "thirdParty" && <ThirdPartyList />}
        <div className="actions">
          <button
            onClick={() =>
              void navigator.clipboard.writeText(summary).then(() => {
                setCopied(true);
                setTimeout(() => setCopied(false), 1500);
              })
            }
          >
            {copied ? "Copiato" : "Copia informazioni"}
          </button>
          <button className="primary" autoFocus onClick={onClose}>
            Chiudi
          </button>
        </div>
      </div>
    </div>
  );
}

/** Loaded on demand: several hundred components with their license texts. */
function ThirdPartyList() {
  const [data, setData] = useState<ThirdParty | null>(null);
  const [query, setQuery] = useState("");
  const [open, setOpen] = useState<string | null>(null);

  useEffect(() => {
    void import("../generated/third-party.json").then((module) => setData(module.default as ThirdParty));
  }, []);

  const shown = useMemo(() => {
    const q = query.trim().toLowerCase();
    return (data?.packages ?? []).filter((p) => !q || `${p.name} ${p.license}`.toLowerCase().includes(q));
  }, [data, query]);

  if (!data) return <p className="muted small">Caricamento…</p>;
  return (
    <div className="third-party">
      <p className="muted small">
        Glassferry include {data.packages.length} componenti di terze parti (librerie Rust e JavaScript), ciascuno con la
        propria licenza. Grazie a chi li sviluppa.
      </p>
      <input
        className="third-party-search"
        placeholder="Cerca per nome o licenza…"
        value={query}
        spellCheck={false}
        onChange={(e) => setQuery(e.target.value)}
      />
      <ul className="third-party-list">
        {shown.map((p) => {
          const key = `${p.source}:${p.name}@${p.version}`;
          return (
            <li key={key}>
              <button className="third-party-row" onClick={() => setOpen(open === key ? null : key)}>
                <span className="mono">
                  {p.name} {p.version}
                </span>
                <span className="muted">{p.license}</span>
              </button>
              {open === key && (
                <div className="third-party-detail small">
                  <p className="muted">
                    {p.source} · {p.url}
                  </p>
                  {p.texts.length > 0 ? (
                    p.texts.map((t) => (
                      <pre key={t} className="license-text">
                        {data.texts[t]}
                      </pre>
                    ))
                  ) : (
                    <p className="muted">
                      Il pacchetto non include il testo della licenza: è quello standard di {p.license}, su spdx.org/licenses.
                    </p>
                  )}
                </div>
              )}
            </li>
          );
        })}
      </ul>
    </div>
  );
}
