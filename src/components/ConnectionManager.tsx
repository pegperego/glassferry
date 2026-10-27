import { open, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { useEffect, useMemo, useState, type ReactNode } from "react";
import {
  connectionCheck,
  connectionForgetPassword,
  connectionsExport,
  connectionsImport,
  listHosts,
  type Category,
  type Connection,
  type HostEntry,
  type Source,
  type Store,
} from "../api";
import {
  addCategory,
  categoryInUse,
  categoryOf,
  connectionErrors,
  describeSource,
  duplicateConnection,
  emptyAppSource,
  groupByCategory,
  importAliases,
  mergeStores,
  newConnection,
  notImported,
  parseOptions,
  removeCategory,
  removeConnection,
  searchConnections,
  upsertConnection,
} from "../connections";
import { ConfirmDialog } from "./Dialogs";

/** Settings shown first in the `ssh -G` summary. */
const KEY_SETTINGS = ["hostname", "user", "port", "identityfile", "proxyjump", "proxycommand", "identityagent"];

type View =
  | { kind: "none" }
  | { kind: "edit"; draft: Connection }
  | { kind: "import" }
  | { kind: "categories" };

interface Props {
  store: Store;
  save: (store: Store) => Promise<Store>;
  home: string | null;
  verbose: Set<string>;
  onToggleVerbose: (connectionId: string) => void;
  /** Open on the import view (first launch, empty list). */
  startWithImport: boolean;
  onConnect: (connectionId: string) => void;
  onClose: () => void;
}

/** "Gestisci connessioni": list, edit, import from ~/.ssh/config, categories, import/export. */
export function ConnectionManager(props: Props) {
  const { store, save, onClose } = props;
  const [filter, setFilter] = useState("");
  const [view, setView] = useState<View>(props.startWithImport ? { kind: "import" } : { kind: "none" });
  const [message, setMessage] = useState<{ text: string; error?: boolean } | null>(null);
  const [pending, setPending] = useState<(() => void) | null>(null);

  const original = view.kind === "edit" ? store.connections.find((c) => c.id === view.draft.id) : undefined;
  const dirty = view.kind === "edit" && JSON.stringify(original) !== JSON.stringify(view.draft);
  /** Runs `action`, asking first when the edited connection has unsaved changes. */
  const guard = (action: () => void) => (dirty ? setPending(() => action) : action());
  const go = (next: View) =>
    guard(() => {
      setMessage(null);
      setView(next);
    });

  const run = async (action: () => Promise<string | void>) => {
    try {
      const text = await action();
      setMessage(text ? { text } : null);
    } catch (e) {
      setMessage({ text: String(e), error: true });
    }
  };

  const groups = groupByCategory(store, searchConnections(store, filter));
  const selectedId = view.kind === "edit" ? view.draft.id : null;
  const defaultCategory = store.categories[0]?.id ?? "";

  const exportFile = () =>
    run(async () => {
      const path = await saveDialog({
        defaultPath: "connessioni-sshfilemanager.json",
        filters: [{ name: "JSON", extensions: ["json"] }],
      });
      if (!path) return;
      await connectionsExport(path);
      return `Esportate ${store.connections.length} connessioni in ${path} (senza password).`;
    });

  const importFile = () =>
    run(async () => {
      const path = await open({ multiple: false, filters: [{ name: "JSON", extensions: ["json"] }] });
      if (typeof path !== "string") return;
      const merged = mergeStores(store, await connectionsImport(path));
      await save(merged.store);
      return `Importate ${merged.added} connessioni${merged.skipped ? `, ${merged.skipped} già presenti` : ""}.`;
    });

  return (
    <div className="modal-backdrop">
      <div className="modal manager" onKeyDown={(e) => e.key === "Escape" && !pending && guard(onClose)}>
        <header className="manager-header">
          <h2>Gestisci connessioni</h2>
          <span className="spacer" />
          <button onClick={() => guard(onClose)}>Chiudi</button>
        </header>
        <div className="manager-body">
          <aside className="manager-list">
            <div className="manager-actions">
              <button onClick={() => go({ kind: "edit", draft: newConnection(defaultCategory) })}>+ Nuova</button>
              <button className={view.kind === "import" ? "toggle on" : ""} onClick={() => go({ kind: "import" })}>
                Importa da ~/.ssh/config
              </button>
              <button className={view.kind === "categories" ? "toggle on" : ""} onClick={() => go({ kind: "categories" })}>
                Categorie
              </button>
            </div>
            <input
              className="manager-filter"
              placeholder={`Cerca tra ${store.connections.length} connessioni…`}
              value={filter}
              onChange={(e) => setFilter(e.target.value)}
            />
            <ul>
              {groups.map((group) => (
                <li key={group.category.id}>
                  <div className="palette-group">
                    <span className="category-dot" style={{ background: group.category.color }} />
                    {group.category.name}
                  </div>
                  <ul>
                    {group.items.map((c) => (
                      <li key={c.id}>
                        <button
                          className={c.id === selectedId ? "manager-item active" : "manager-item"}
                          onClick={() => c.id !== selectedId && go({ kind: "edit", draft: structuredClone(c) })}
                          onDoubleClick={() => !dirty && props.onConnect(c.id)}
                        >
                          <span className="palette-name">{c.name}</span>
                          {describeSource(c.source) !== c.name && <span className="muted small">{describeSource(c.source)}</span>}
                        </button>
                      </li>
                    ))}
                  </ul>
                </li>
              ))}
            </ul>
            <div className="manager-actions">
              <button onClick={() => guard(() => void importFile())}>Importa file…</button>
              <button onClick={() => void exportFile()} disabled={store.connections.length === 0}>
                Esporta file…
              </button>
            </div>
          </aside>
          <section className="manager-detail">
            {message && (
              <p className={message.error ? "error op-error" : "manager-message"}>
                {message.text}
                <button className="inline" onClick={() => setMessage(null)}>
                  ✕
                </button>
              </p>
            )}
            {view.kind === "none" && (
              <p className="muted">
                Scegli una connessione a sinistra, creane una nuova o importa gli host di ~/.ssh/config.
              </p>
            )}
            {view.kind === "import" && (
              <ImportView
                store={store}
                onImport={(aliases, categoryId) =>
                  run(async () => {
                    await save(importAliases(store, aliases, categoryId));
                    return `Importate ${aliases.length} connessioni.`;
                  })
                }
              />
            )}
            {view.kind === "categories" && (
              <CategoriesView
                store={store}
                onSave={(categories) =>
                  run(async () => {
                    await save({ ...store, categories });
                    return "Categorie salvate.";
                  })
                }
              />
            )}
            {view.kind === "edit" && (
              <ConnectionForm
                key={view.draft.id}
                draft={view.draft}
                setDraft={(draft) => setView({ kind: "edit", draft })}
                store={store}
                home={props.home}
                isNew={!original}
                dirty={dirty}
                savedPassword={original?.savePassword ?? false}
                verbose={props.verbose.has(view.draft.id)}
                onToggleVerbose={() => props.onToggleVerbose(view.draft.id)}
                onSave={() =>
                  run(async () => {
                    await save(upsertConnection(store, view.draft));
                    return "Connessione salvata.";
                  })
                }
                onRevert={() => (original ? setView({ kind: "edit", draft: structuredClone(original) }) : setView({ kind: "none" }))}
                onDuplicate={() => go({ kind: "edit", draft: duplicateConnection(view.draft) })}
                onDelete={() =>
                  run(async () => {
                    await save(removeConnection(store, view.draft.id));
                    setView({ kind: "none" });
                    return `«${view.draft.name}» eliminata.`;
                  })
                }
                onForgetPassword={() =>
                  run(async () => {
                    await connectionForgetPassword(view.draft.id);
                    return "Password rimossa dal Portachiavi.";
                  })
                }
                onConnect={() => props.onConnect(view.draft.id)}
              />
            )}
          </section>
        </div>
        {pending && (
          <ConfirmDialog
            title="Modifiche non salvate"
            confirmLabel="Scarta modifiche"
            danger
            onCancel={() => setPending(null)}
            onConfirm={() => {
              const action = pending;
              setPending(null);
              setView({ kind: "none" });
              action();
            }}
          >
            <p>La connessione che stai modificando ha modifiche non salvate.</p>
          </ConfirmDialog>
        )}
      </div>
    </div>
  );
}

interface FormProps {
  draft: Connection;
  setDraft: (draft: Connection) => void;
  store: Store;
  home: string | null;
  isNew: boolean;
  dirty: boolean;
  savedPassword: boolean;
  verbose: boolean;
  onToggleVerbose: () => void;
  onSave: () => void;
  onRevert: () => void;
  onDuplicate: () => void;
  onDelete: () => void;
  onForgetPassword: () => void;
  onConnect: () => void;
}

type Effective = { state: "idle" } | { state: "loading" } | { state: "ok"; values: Record<string, string> } | { state: "error"; error: string };

function ConnectionForm(props: FormProps) {
  const { draft, setDraft, store } = props;
  const [hosts, setHosts] = useState<HostEntry[]>([]);
  const [effective, setEffective] = useState<Effective>({ state: "idle" });
  const [confirmDelete, setConfirmDelete] = useState(false);
  // Raw text: options are parsed on every change, but blank lines must survive typing.
  const [optionsText, setOptionsText] = useState(draft.source.kind === "app" ? draft.source.extraOptions.join("\n") : "");
  const errors = connectionErrors(draft, store);
  const valid = Object.keys(errors).length === 0;
  const source = draft.source;

  useEffect(() => {
    listHosts().then(setHosts, () => setHosts([]));
  }, []);

  const check = (s: Source) => {
    setEffective({ state: "loading" });
    connectionCheck(s).then(
      (values) => setEffective({ state: "ok", values }),
      (e) => setEffective({ state: "error", error: String(e) }),
    );
  };
  // ssh config aliases are resolved right away; app sources on request.
  const alias = source.kind === "sshConfig" ? source.alias : null;
  useEffect(() => {
    if (alias) check({ kind: "sshConfig", alias });
    else setEffective({ state: "idle" });
  }, [alias]);

  const setSource = (s: Source) => {
    setDraft({ ...draft, source: s });
    if (s.kind === "app") setEffective({ state: "idle" });
  };
  const app = source.kind === "app" ? source : null;
  const setApp = (patch: Partial<Extract<Source, { kind: "app" }>>) => app && setSource({ ...app, ...patch });
  const nullable = (v: string) => (v.trim() === "" ? null : v);

  const pickFile = async (directory: boolean, defaultPath?: string) => {
    const picked = await open({ directory, multiple: false, defaultPath });
    return typeof picked === "string" ? picked : null;
  };
  const aliasMissing = alias !== null && hosts.length > 0 && !hosts.some((h) => h.alias === alias);
  const jumpTargets = useMemo(
    () => [
      ...hosts.map((h) => ({ value: h.alias, label: "~/.ssh/config" })),
      ...store.connections
        .filter((c) => c.source.kind === "app" && c.id !== draft.id)
        .map((c) => ({ value: `sfm-${c.id}`, label: c.name })),
    ],
    [hosts, store, draft.id],
  );

  return (
    <div className="connection-form">
      <div className="form-grid">
        <Field label="Nome" error={errors.name}>
          <input autoFocus={props.isNew} value={draft.name} onChange={(e) => setDraft({ ...draft, name: e.target.value })} />
        </Field>
        <Field label="Categoria" error={errors.categoryId}>
          <div className="category-select">
            <span className="category-dot" style={{ background: categoryOf(store, draft)?.color }} />
            <select value={draft.categoryId} onChange={(e) => setDraft({ ...draft, categoryId: e.target.value })}>
              {store.categories.map((c) => (
                <option key={c.id} value={c.id}>
                  {c.name}
                  {c.protected ? " (protetta)" : ""}
                </option>
              ))}
            </select>
          </div>
        </Field>
        <div className="field">
          Impostazioni ssh
          <div className="segmented">
            <button
              className={source.kind === "sshConfig" ? "toggle on" : ""}
              onClick={() => source.kind !== "sshConfig" && setSource({ kind: "sshConfig", alias: hosts[0]?.alias ?? "" })}
            >
              Da ~/.ssh/config
            </button>
            <button
              className={source.kind === "app" ? "toggle on" : ""}
              onClick={() => {
                if (source.kind === "app") return;
                setOptionsText("");
                setSource(emptyAppSource());
              }}
            >
              Configurate qui
            </button>
          </div>
        </div>

        {source.kind === "sshConfig" && (
          <Field label="Alias" error={errors.alias ?? (aliasMissing ? "Alias non trovato in ~/.ssh/config" : undefined)}>
            <select value={source.alias} onChange={(e) => setSource({ kind: "sshConfig", alias: e.target.value })}>
              {aliasMissing && <option value={source.alias}>{source.alias} (non trovato)</option>}
              {hosts.map((h) => (
                <option key={h.alias} value={h.alias}>
                  {h.alias}
                </option>
              ))}
            </select>
          </Field>
        )}

        {app && (
          <>
            <Field label="Host" error={errors.host}>
              <input value={app.host} placeholder="server.example.com" onChange={(e) => setApp({ host: e.target.value.trim() })} />
            </Field>
            <div className="form-row">
              <Field label="Utente" error={errors.user}>
                <input value={app.user ?? ""} placeholder="dal config" onChange={(e) => setApp({ user: nullable(e.target.value.trim()) })} />
              </Field>
              <Field label="Porta" error={errors.port}>
                <input
                  value={app.port ?? ""}
                  placeholder="22"
                  inputMode="numeric"
                  onChange={(e) => setApp({ port: e.target.value.trim() === "" ? null : Number(e.target.value) })}
                />
              </Field>
            </div>
            <Field label="Chiave (IdentityFile)" error={errors.identityFile} group>
              <div className="input-with-button">
                <input
                  aria-label="Chiave (IdentityFile)"
                  value={app.identityFile ?? ""}
                  placeholder="agent o chiavi di default"
                  onChange={(e) => setApp({ identityFile: nullable(e.target.value) })}
                />
                <button
                  onClick={() =>
                    void pickFile(false, props.home ? `${props.home}/.ssh` : undefined).then((p) => p && setApp({ identityFile: p }))
                  }
                >
                  Scegli…
                </button>
              </div>
            </Field>
            <Field label="ProxyJump" error={errors.proxyJump}>
              <input
                list="jump-targets"
                value={app.proxyJump ?? ""}
                placeholder="nessuno"
                onChange={(e) => setApp({ proxyJump: nullable(e.target.value.trim()) })}
              />
              <datalist id="jump-targets">
                {jumpTargets.map((t) => (
                  <option key={t.value} value={t.value}>
                    {t.label}
                  </option>
                ))}
              </datalist>
            </Field>
            <Field label="Opzioni aggiuntive (una per riga: Chiave valore)" error={errors.extraOptions}>
              <textarea
                rows={3}
                spellCheck={false}
                value={optionsText}
                placeholder="ServerAliveInterval 30"
                onChange={(e) => {
                  setOptionsText(e.target.value);
                  setApp({ extraOptions: parseOptions(e.target.value) });
                }}
              />
            </Field>
          </>
        )}

        <div className="form-row">
          <Field label="Cartella remota iniziale">
            <input
              value={draft.remoteDir ?? ""}
              placeholder="home"
              onChange={(e) => setDraft({ ...draft, remoteDir: nullable(e.target.value) })}
            />
          </Field>
          <Field label="Cartella locale iniziale" group>
            <div className="input-with-button">
              <input
                aria-label="Cartella locale iniziale"
                value={draft.localDir ?? ""}
                placeholder="invariata"
                onChange={(e) => setDraft({ ...draft, localDir: nullable(e.target.value) })}
              />
              <button onClick={() => void pickFile(true, props.home ?? undefined).then((p) => p && setDraft({ ...draft, localDir: p }))}>
                Scegli…
              </button>
            </div>
          </Field>
        </div>
        <label className="checkbox">
          <input
            type="checkbox"
            checked={draft.savePassword}
            onChange={(e) => setDraft({ ...draft, savePassword: e.target.checked })}
          />
          Salva la password nel Portachiavi
          {props.savedPassword && !props.dirty && (
            <button className="inline link" onClick={props.onForgetPassword}>
              dimentica password salvata
            </button>
          )}
        </label>
        <label className="checkbox">
          <input type="checkbox" checked={props.verbose} onChange={props.onToggleVerbose} />
          Modalità diagnostica (ssh -v nel log)
        </label>
      </div>

      <div className="form-actions">
        {!props.isNew && (
          <>
            <button className="danger-outline" onClick={() => setConfirmDelete(true)}>
              Elimina
            </button>
            <button onClick={props.onDuplicate}>Duplica</button>
          </>
        )}
        <span className="spacer" />
        {props.dirty && <button onClick={props.onRevert}>{props.isNew ? "Annulla" : "Annulla modifiche"}</button>}
        <button onClick={props.onSave} disabled={!valid || !props.dirty} className={props.dirty ? "primary" : ""}>
          Salva
        </button>
        <button onClick={props.onConnect} disabled={props.dirty || props.isNew} title={props.dirty ? "Salva prima di connetterti" : ""}>
          Connetti
        </button>
      </div>

      <section className="effective">
        <header>
          <strong>Impostazioni effettive</strong>
          <span className="muted small">ssh -G{app ? " sul file generato" : ""}</span>
          <span className="spacer" />
          {app && (
            <button onClick={() => check(app)} disabled={!valid}>
              Verifica
            </button>
          )}
        </header>
        <EffectiveSettings effective={effective} />
      </section>

      {confirmDelete && (
        <ConfirmDialog
          title="Eliminare la connessione?"
          confirmLabel="Elimina"
          danger
          onCancel={() => setConfirmDelete(false)}
          onConfirm={() => {
            setConfirmDelete(false);
            props.onDelete();
          }}
        >
          <p>
            «{draft.name}» viene tolta dall'elenco
            {draft.savePassword ? " e la password salvata viene rimossa dal Portachiavi" : ""}.
            {draft.source.kind === "sshConfig" && " Il tuo ~/.ssh/config non viene modificato."}
          </p>
        </ConfirmDialog>
      )}
    </div>
  );
}

function EffectiveSettings({ effective }: { effective: Effective }) {
  if (effective.state === "idle") return <p className="muted small">Premi "Verifica" per controllare le impostazioni con ssh.</p>;
  if (effective.state === "loading") return <p className="muted small">ssh -G…</p>;
  if (effective.state === "error") return <pre className="error-text small effective-error">{effective.error}</pre>;
  const { values } = effective;
  const rest = Object.keys(values).filter((k) => !KEY_SETTINGS.includes(k));
  const row = (k: string) => (
    <tr key={k}>
      <th>{k}</th>
      <td>{values[k]}</td>
    </tr>
  );
  return (
    <>
      <table className="effective-table">
        <tbody>{KEY_SETTINGS.filter((k) => k in values).map(row)}</tbody>
      </table>
      <details>
        <summary className="muted small">Tutte le impostazioni ({rest.length})</summary>
        <table className="effective-table">
          <tbody>{rest.map(row)}</tbody>
        </table>
      </details>
    </>
  );
}

/** A label wraps the control; with `group` (control plus buttons) a div is used instead. */
function Field({ label, error, group, children }: { label: string; error?: string; group?: boolean; children: ReactNode }) {
  const content = (
    <>
      {label}
      {children}
      {error && <span className="error-text small">{error}</span>}
    </>
  );
  return group ? <div className="field">{content}</div> : <label className="field">{content}</label>;
}

function ImportView({ store, onImport }: { store: Store; onImport: (aliases: string[], categoryId: string) => void }) {
  const [hosts, setHosts] = useState<HostEntry[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [chosen, setChosen] = useState<Set<string>>(new Set());
  const [categoryId, setCategoryId] = useState(store.categories[0]?.id ?? "");
  useEffect(() => {
    listHosts().then(setHosts, (e) => setError(String(e)));
  }, []);
  const available = hosts ? notImported(hosts, store) : [];
  const toggle = (alias: string) => {
    const next = new Set(chosen);
    if (next.has(alias)) next.delete(alias);
    else next.add(alias);
    setChosen(next);
  };
  if (error) return <p className="error">{error}</p>;
  if (!hosts) return <p className="muted">Lettura di ~/.ssh/config…</p>;
  return (
    <div className="import-view">
      <h3>Importa da ~/.ssh/config</h3>
      <p className="muted small">
        Le impostazioni restano nel tuo config: l'app salva solo l'alias, il nome e la categoria.
      </p>
      {available.length === 0 ? (
        <p className="muted">Tutti gli host di ~/.ssh/config sono già nell'elenco.</p>
      ) : (
        <>
          <label className="checkbox">
            <input
              type="checkbox"
              checked={chosen.size === available.length}
              onChange={(e) => setChosen(e.target.checked ? new Set(available.map((h) => h.alias)) : new Set())}
            />
            Seleziona tutti ({available.length})
          </label>
          <ul className="import-list">
            {available.map((h) => (
              <li key={h.alias}>
                <label className="checkbox">
                  <input type="checkbox" checked={chosen.has(h.alias)} onChange={() => toggle(h.alias)} />
                  {h.alias}
                  <span className="muted small">{h.source}</span>
                </label>
              </li>
            ))}
          </ul>
          <div className="form-actions">
            <label className="field inline-field">
              Categoria
              <select value={categoryId} onChange={(e) => setCategoryId(e.target.value)}>
                {store.categories.map((c) => (
                  <option key={c.id} value={c.id}>
                    {c.name}
                  </option>
                ))}
              </select>
            </label>
            <span className="spacer" />
            <button
              className="primary"
              disabled={chosen.size === 0}
              onClick={() => {
                onImport(available.filter((h) => chosen.has(h.alias)).map((h) => h.alias), categoryId);
                setChosen(new Set());
              }}
            >
              Importa {chosen.size || ""}
            </button>
          </div>
        </>
      )}
    </div>
  );
}

function CategoriesView({ store, onSave }: { store: Store; onSave: (categories: Category[]) => void }) {
  const [draft, setDraft] = useState(store.categories);
  const draftStore = { ...store, categories: draft };
  const set = (id: string, patch: Partial<Category>) => setDraft(draft.map((c) => (c.id === id ? { ...c, ...patch } : c)));
  const dirty = JSON.stringify(draft) !== JSON.stringify(store.categories);
  const valid = draft.every((c) => c.name.trim() !== "");
  return (
    <div className="categories-view">
      <h3>Categorie</h3>
      <p className="muted small">
        Il colore identifica la connessione nei pannelli e nei dialoghi. Sulle categorie protette le operazioni distruttive
        chiedono una conferma in più.
      </p>
      <table className="categories-table">
        <tbody>
          {draft.map((c) => {
            const used = categoryInUse(draftStore, c.id);
            return (
              <tr key={c.id}>
                <td>
                  <input type="color" value={c.color} onChange={(e) => set(c.id, { color: e.target.value })} />
                </td>
                <td>
                  <input value={c.name} onChange={(e) => set(c.id, { name: e.target.value })} />
                </td>
                <td>
                  <label className="checkbox">
                    <input type="checkbox" checked={c.protected} onChange={(e) => set(c.id, { protected: e.target.checked })} />
                    protetta
                  </label>
                </td>
                <td className="muted small">{used} connessioni</td>
                <td>
                  <button
                    disabled={used > 0}
                    title={used > 0 ? "Sposta prima le connessioni in un'altra categoria" : "Elimina categoria"}
                    onClick={() => setDraft(removeCategory(draftStore, c.id).categories)}
                  >
                    ✕
                  </button>
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
      <div className="form-actions">
        <button onClick={() => setDraft(addCategory(draftStore, "Nuova categoria", "#5b5fc7").categories)}>+ Categoria</button>
        <span className="spacer" />
        {dirty && <button onClick={() => setDraft(store.categories)}>Annulla modifiche</button>}
        <button className={dirty ? "primary" : ""} disabled={!dirty || !valid} onClick={() => onSave(draft)}>
          Salva categorie
        </button>
      </div>
    </div>
  );
}
