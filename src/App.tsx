import { getCurrentWebview } from "@tauri-apps/api/webview";
import { useEffect, useRef, useState, type ReactElement } from "react";
import {
  askpassRespond,
  fsMkdir,
  openTerminal,
  onMenu,
  fsAttrs,
  fsDirSize,
  fsSearch,
  fsCompress,
  fsExtract,
  fsSearchCancel,
  fsMove,
  fsPlan,
  fsPlanDiscard,
  fsPlanExecute,
  fsRename,
  localHome,
  localListDir,
  localOpen,
  onAskpassRequest,
  type AskpassRequest,
  type Connection,
  type Direction,
  type EditInfo,
  type JobInfo,
  type Target,
} from "./api";
import { AskpassDialog } from "./components/AskpassDialog";
import { BottomPanel, type BottomTab } from "./components/BottomPanel";
import { ConflictDialog } from "./components/ConflictDialog";
import { ConnectionManager } from "./components/ConnectionManager";
import { DragGhost } from "./components/DragGhost";
import { ChooseDialog, ConfirmDialog } from "./components/Dialogs";
import { EditConfirmDialog } from "./components/EditConfirmDialog";
import { EditConflictDialog } from "./components/EditConflictDialog";
import { FilePane, type PaneOps } from "./components/FilePane";
import { QuickConnect } from "./components/QuickConnect";
import { SettingsDialog } from "./components/SettingsDialog";
import { AboutDialog } from "./components/AboutDialog";
import { RemotePane } from "./components/RemotePane";
import type { PaneBookmarks } from "./components/FilePane";
import { SplitView } from "./components/SplitView";
import { Toolbar, type RemoteLayout } from "./components/Toolbar";
import {
  addBookmark,
  bookmarksOf,
  categoryOf,
  findBookmark,
  protectionOf,
  removeBookmark,
  type Protection,
} from "./connections";
import { groupByParent, remoteSide, slotOf, type DragPayload, type Side } from "./dnd";
import { usePersistentState } from "./settings";
import { useActivityLog } from "./useActivityLog";
import { useConnections } from "./useConnections";
import { useEdits } from "./useEdits";
import { usePane, type PaneState } from "./usePane";
import { sessionOf, useRemoteSlots } from "./useRemoteSlots";
import { useTransfers } from "./useTransfers";
import "./App.css";

type Modal =
  | { kind: "quick" }
  | { kind: "settings" }
  | { kind: "about" }
  | { kind: "manager"; startWithImport: boolean }
  /** Both remote panes are in use: which one does `connection` replace? */
  | { kind: "replace"; connection: Connection; startPath?: string; only?: number }
  /** Closing `slot` detaches open files with changes not uploaded yet: `then` closes it. */
  | { kind: "disconnect"; slot: number; files: EditInfo[]; then: () => void }
  | null;

/** What the account allows beyond SFTP: a shell (remote terminal), chown/chgrp, tar/unzip. */
function paneOps(
  target: Target,
  protection: Protection | null,
  can: { shell: boolean; chown: boolean; tar: boolean; unzip: boolean },
): PaneOps {
  return {
    mkdir: (dir, name) => fsMkdir(target, dir, name),
    rename: (dir, from, to) => fsRename(target, dir, from, to),
    move: (srcDir, names, dstDir) => fsMove(target, srcDir, names, dstDir),
    plan: (request) => fsPlan(target, request),
    execute: (id) => fsPlanExecute(id),
    discard: (id) => void fsPlanDiscard(id).catch(() => {}),
    permanentDelete: target.kind === "remote",
    protection,
    openTerminal: can.shell ? (dir) => openTerminal(target, dir) : null,
    attrs: (dir, names, change, dryRun) => fsAttrs(target, dir, names, change, dryRun),
    dirSize: (dir, names) => fsDirSize(target, dir, names),
    search: (dir, query, id) => fsSearch(target, dir, query, id),
    compress: can.tar ? (dir, names, archive) => fsCompress(target, dir, names, archive) : null,
    extract: (name) =>
      (name.toLowerCase().endsWith(".zip") ? can.unzip : can.tar) ? (dir) => fsExtract(target, dir, name) : null,
    cancelSearch: (id) => void fsSearchCancel(id).catch(() => {}),
    canChown: can.chown,
  };
}

const localOps = paneOps({ kind: "local" }, null, { shell: true, chown: false, tar: true, unzip: true });

/** `dir` is `path` or inside it. */
const within = (path: string | null, dir: string) =>
  path !== null && (dir === path || dir.startsWith(`${path.replace(/\/$/, "")}/`));

export default function App() {
  const connections = useConnections();
  const store = connections.store;
  const [modal, setModal] = useState<Modal>(null);
  const [prompts, setPrompts] = useState<AskpassRequest[]>([]);
  const [home, setHome] = useState<string | null>(null);
  const [activeSide, setActiveSide] = useState<Side>("local");
  /** Remote slot used last: default destination of uploads (F5 in the local pane). */
  const [lastRemote, setLastRemote] = useState(0);
  const [layout, setLayout] = usePersistentState<RemoteLayout>("layout.remotes", "side");
  const [splitSide2, setSplitSide2] = usePersistentState<number[]>("split.side.2", []);
  const [splitSide3, setSplitSide3] = usePersistentState<number[]>("split.side.3", []);
  const [splitOuter, setSplitOuter] = usePersistentState<number[]>("split.stacked.outer", []);
  const [splitInner, setSplitInner] = usePersistentState<number[]>("split.stacked.inner", []);
  const [panelOpen, setPanelOpen] = usePersistentState("log.open", true);
  const [panelHeight, setPanelHeight] = usePersistentState("log.height", 200);
  const [panelTab, setPanelTab] = usePersistentState<BottomTab>("panel.tab", "log");
  const [verboseList, setVerboseList] = usePersistentState<string[]>("connections.verbose", []);
  const verboseHosts = new Set(verboseList);
  const log = useActivityLog(panelOpen && panelTab === "log");
  const [appError, setAppError] = useState<string | null>(null);
  const edits = useEdits((message) => setAppError(message));

  const localPane = usePane(localListDir, home);
  const remote = useRemoteSlots((id) => verboseHosts.has(id), edits.refresh);
  const { slots, panes } = remote;
  const connected = [0, 1].filter((i) => slots[i].state === "connected");

  // Refresh the destination pane(s) when a transfer into their directory ends.
  const panesRef = useRef({ localPane, panes, slots });
  panesRef.current = { localPane, panes, slots };
  const transfers = useTransfers((job: JobInfo) => {
    const current = panesRef.current;
    if (job.direction === "download") {
      if (within(current.localPane.path, job.dstDir)) void current.localPane.refresh();
      return;
    }
    const target = job.direction === "remote" ? job.dstSessionId : job.sessionId;
    current.panes.forEach((pane, i) => {
      if (sessionOf(current.slots[i])?.sessionId === target && within(pane.path, job.dstDir)) void pane.refresh();
    });
  });

  const startTransfer = (slot: number, direction: Direction, srcDir: string, names: string[], dstDir: string) => {
    const session = sessionOf(slots[slot]);
    if (!session || names.length === 0) return;
    setLastRemote(slot);
    void transfers.start(session.sessionId, direction, srcDir, names, dstDir);
    setPanelTab("transfers");
    if (!panelOpen) setPanelOpen(true);
  };

  /** Copy between the two remote panes (server-side within one session, streamed otherwise). */
  const startRemoteCopy = (from: number, to: number, srcDir: string, names: string[], dstDir: string) => {
    const src = sessionOf(slots[from]);
    const dst = sessionOf(slots[to]);
    if (!src || !dst || names.length === 0) return;
    transfers.start(src.sessionId, "remote", srcDir, names, dstDir, dst.sessionId).catch((e) => setAppError(String(e)));
    setPanelTab("transfers");
    if (!panelOpen) setPanelOpen(true);
  };

  const togglePanel = () => {
    if (!panelOpen) log.markSeen();
    setPanelOpen(!panelOpen);
  };

  const toggleVerbose = (id: string) =>
    setVerboseList(verboseHosts.has(id) ? verboseList.filter((a) => a !== id) : [...verboseList, id]);

  const activate = (side: Side) => {
    setActiveSide(side);
    const slot = slotOf(side);
    if (slot !== null) setLastRemote(slot);
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.metaKey && e.key.toLowerCase() === "j") {
        e.preventDefault();
        togglePanel();
      } else if (e.metaKey && e.key.toLowerCase() === "k" && !prompts[0]) {
        e.preventDefault();
        setModal({ kind: "quick" });
      } else if (e.metaKey && e.key === "," && !prompts[0]) {
        e.preventDefault();
        setModal({ kind: "settings" });
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  // macOS menu bar: "Informazioni su Glassferry", "Impostazioni…".
  useEffect(() => {
    const unlisten = onMenu((id) => {
      if (id === "about" || id === "settings") setModal({ kind: id });
    });
    return () => void unlisten.then((fn) => fn());
  }, []);

  useEffect(() => {
    localHome().then(setHome, () => setHome("/"));
  }, []);

  // First launch: nothing to connect to yet, start from the import.
  const offeredImport = useRef(false);
  useEffect(() => {
    if (!store || offeredImport.current) return;
    offeredImport.current = true;
    if (store.connections.length === 0) setModal({ kind: "manager", startWithImport: true });
  }, [store]);

  useEffect(() => {
    const unlisten = onAskpassRequest((request) =>
      setPrompts((queue) => (queue.some((p) => p.id === request.id) ? queue : [...queue, request])),
    );
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, []);

  // Files dropped from the Finder onto a remote pane are uploaded there.
  const finderDropRef = useRef<(paths: string[], x: number, y: number) => void>(() => {});
  finderDropRef.current = (paths, x, y) => {
    const target = document.elementFromPoint(x, y)?.closest("[data-pane]");
    const slot = slotOf((target?.getAttribute("data-pane") ?? "local") as Side);
    const dir = slot === null ? null : panes[slot].path;
    if (slot === null || dir === null) return;
    for (const [src, names] of groupByParent(paths)) startTransfer(slot, "upload", src, names, dir);
  };
  useEffect(() => {
    const unlisten = getCurrentWebview().onDragDropEvent((event) => {
      if (event.payload.type !== "drop") return;
      const ratio = window.devicePixelRatio || 1;
      const { x, y } = event.payload.position;
      finderDropRef.current(event.payload.paths, x / ratio, y / ratio);
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, []);

  const answerPrompt = (value: string | null, save = false) => {
    const [current, ...rest] = prompts;
    if (!current) return;
    setPrompts(rest);
    if (current.kind !== "notice") void askpassRespond(current.id, value, save);
  };

  const openIn = async (slot: number, connection: Connection, startPath?: string) => {
    setModal(null);
    const result = await remote.open(slot, connection, startPath);
    if (!result) return;
    activate(remoteSide(slot));
    if (connection.localDir && startPath === undefined) void localPane.navigate(connection.localDir);
  };

  /**
   * Runs `action`, which closes `slot`. When that disconnects a session (the
   * other pane does not share it) with open files not uploaded yet, asks first.
   */
  const guardClose = (slot: number, action: () => void) => {
    const session = sessionOf(slots[slot]);
    const shared = session !== null && sessionOf(slots[1 - slot])?.sessionId === session.sessionId;
    const files =
      session && !shared ? edits.edits.filter((e) => e.sessionId === session.sessionId && e.state !== "synced") : [];
    if (files.length === 0) action();
    else setModal({ kind: "disconnect", slot, files, then: action });
  };

  /** Session open for a connection in one of the remote panes. */
  const sessionFor = (connectionId: string) => {
    for (const slot of slots) {
      if (slot.state === "connected" && slot.connection.id === connectionId) return slot.result.sessionId;
    }
    return null;
  };

  /** New connection: a free remote pane, or ask which one to replace. */
  const handleConnect = (connectionId: string) => {
    const connection = store?.connections.find((c) => c.id === connectionId);
    if (!connection) return;
    const free = [0, 1].find((i) => slots[i].state === "idle" || slots[i].state === "failed");
    if (free !== undefined) void openIn(free, connection);
    else setModal({ kind: "replace", connection });
  };

  /** A bookmark of a connection: the pane already showing it goes there, else it opens like a new connection. */
  const openBookmark = (connectionId: string, path: string) => {
    setModal(null);
    const open = [0, 1].find((i) => {
      const s = slots[i];
      return s.state === "connected" && s.connection.id === connectionId;
    });
    if (open !== undefined) {
      void panes[open].navigate(path);
      activate(remoteSide(open));
      return;
    }
    const connection = store?.connections.find((c) => c.id === connectionId);
    if (!connection) return;
    const free = [0, 1].find((i) => slots[i].state === "idle" || slots[i].state === "failed");
    if (free !== undefined) void openIn(free, connection, path);
    else setModal({ kind: "replace", connection, startPath: path });
  };

  /** ★ menu and ⌘D of a pane: `connectionId` null for the local one. */
  const paneBookmarks = (connectionId: string | null): PaneBookmarks | undefined =>
    store
      ? {
          list: bookmarksOf(store, connectionId),
          toggle: (path) => {
            const existing = findBookmark(store, connectionId, path);
            const next = existing ? removeBookmark(store, existing.id) : addBookmark(store, connectionId, path);
            connections.save(next).catch((e) => setAppError(String(e)));
          },
        }
      : undefined;

  /** "Apri nell'altro pannello": same connection and folder in the other remote pane. */
  const openInOther = (slot: number) => {
    const source = slots[slot];
    if (source.state !== "connected") return;
    const other = 1 - slot;
    const startPath = panes[slot].path ?? undefined;
    if (slots[other].state === "idle" || slots[other].state === "failed") void openIn(other, source.connection, startPath);
    else setModal({ kind: "replace", connection: source.connection, startPath, only: other });
  };

  const onDropToRemote = (slot: number) => (payload: DragPayload, targetDir: string) => {
    const from = slotOf(payload.side);
    if (from === null) startTransfer(slot, "upload", payload.dir, payload.names, targetDir);
    else if (from !== slot) startRemoteCopy(from, slot, payload.dir, payload.names, targetDir);
  };
  const onDropToLocal = (payload: DragPayload, targetDir: string) => {
    const slot = slotOf(payload.side);
    if (slot !== null) startTransfer(slot, "download", payload.dir, payload.names, targetDir);
  };

  const nameOf = (slot: number) => {
    const s = slots[slot];
    return s.state === "idle" ? "" : s.connection.name;
  };
  // Two panes on the same connection are told apart by their number.
  const sameConnection =
    slots[0].state !== "idle" && slots[1].state !== "idle" && slots[0].connection.id === slots[1].connection.id;
  const paneName = (slot: number) => (sameConnection ? `${nameOf(slot)} (${slot + 1})` : nameOf(slot));

  const uploadTargets = connected
    .filter((i) => panes[i].path !== null && localPane.path !== null)
    .map((i) => ({
      slot: i,
      label: `Carica su ${paneName(i)} →`,
      run: (names: string[]) => startTransfer(i, "upload", localPane.path!, names, panes[i].path!),
    }));
  const defaultUpload = uploadTargets.find((t) => t.slot === lastRemote) ?? uploadTargets[0];

  /** Protection of a host (connection name) shown in transfer and edit prompts. */
  const hostProtection = (host: string): Protection | null => {
    const connection = store?.connections.find((c) => c.name === host);
    return connection ? protectionOf(store, connection) : null;
  };

  const hostColor = (host: string) => {
    const connection = store?.connections.find((c) => c.name === host);
    return store && connection ? categoryOf(store, connection)?.color : undefined;
  };

  const localView = (
    <FilePane
      key="local"
      pane={localPane}
      side="local"
      ops={localOps}
      bookmarks={paneBookmarks(null)}
      title="Locale"
      active={activeSide === "local"}
      onActivate={() => activate("local")}
      transfer={
        defaultUpload && {
          label: defaultUpload.label,
          title: `Carica su ${paneName(defaultUpload.slot)}:${panes[defaultUpload.slot].path}`,
          run: defaultUpload.run,
        }
      }
      transferTargets={uploadTargets}
      acceptsFrom={connected.map(remoteSide)}
      onDropFiles={onDropToLocal}
      openFile={{ label: "Apri con l'app predefinita", run: (path) => void localOpen(path).catch((e) => setAppError(String(e))) }}
    />
  );

  /** The other remote pane is connected and both have a folder: remote copies possible. */
  const otherReady = (slot: number) =>
    slots[1 - slot].state === "connected" && panes[1 - slot].path !== null && panes[slot].path !== null;

  const remoteView = (slot: number): ReactElement => {
    const s = slots[slot];
    const pane: PaneState = panes[slot];
    const key = `remote-${slot}`;
    if (s.state === "connected") {
      const download =
        pane.path !== null && localPane.path !== null
          ? {
              label: "← Scarica",
              title: `Scarica in ${localPane.path}`,
              run: (names: string[]) => startTransfer(slot, "download", pane.path!, names, localPane.path!),
            }
          : undefined;
      return (
        <RemotePane
          key={key}
          side={remoteSide(slot)}
          bookmarks={paneBookmarks(s.connection.id)}
          ops={paneOps(
            { kind: "remote", sessionId: s.result.sessionId },
            protectionOf(store, s.connection),
            {
              shell: s.result.capabilities.shell,
              chown: ["chown", "chgrp"].every((c) => s.result.capabilities.commands.includes(c)),
              tar: s.result.capabilities.commands.includes("tar"),
              unzip: s.result.capabilities.commands.includes("unzip"),
            },
          )}
          connection={s.connection}
          name={paneName(slot)}
          category={store ? categoryOf(store, s.connection) : undefined}
          session={s.result}
          pane={pane}
          active={activeSide === remoteSide(slot)}
          onActivate={() => activate(remoteSide(slot))}
          onDisconnect={() => guardClose(slot, () => void remote.close(slot))}
          onOpenInOther={() => openInOther(slot)}
          transfer={download}
          transferTargets={[
            ...(download ? [{ label: "Scarica in locale", run: download.run }] : []),
            ...(otherReady(slot)
              ? [
                  {
                    label: `Copia su ${paneName(1 - slot)}`,
                    run: (names: string[]) => startRemoteCopy(slot, 1 - slot, pane.path!, names, panes[1 - slot].path!),
                  },
                ]
              : []),
          ]}
          acceptsFrom={otherReady(slot) ? ["local", remoteSide(1 - slot)] : ["local"]}
          onDropFiles={onDropToRemote(slot)}
          openFile={{
            label: "Modifica",
            run: (path) => {
              void edits.open(s.result.sessionId, path, protectionOf(store, s.connection).protected, s.connection.id);
              setPanelTab("edits");
              if (!panelOpen) setPanelOpen(true);
            },
          }}
        />
      );
    }
    return (
      <section key={key} className="pane placeholder">
        {s.state === "idle" && (
          <p>
            <button className="primary" onClick={() => setModal({ kind: "quick" })}>
              + Nuovo collegamento
            </button>{" "}
            <kbd>⌘K</kbd>
          </p>
        )}
        {s.state === "connecting" && (
          <p>
            Connessione a {s.connection.name}…{" "}
            <button onClick={() => void remote.close(slot)}>Annulla</button>
          </p>
        )}
        {s.state === "failed" && (
          <div className="error">
            <strong>Connessione a {s.connection.name} fallita</strong>
            <pre>{s.error}</pre>
            <p>
              <button onClick={() => void openIn(slot, s.connection)}>Riprova</button>{" "}
              <button onClick={() => void remote.close(slot)}>Chiudi</button>
            </p>
          </div>
        )}
      </section>
    );
  };

  // Remote panes in use; with none, one placeholder.
  const used = [0, 1].filter((i) => slots[i].state !== "idle");
  const remoteViews = (used.length > 0 ? used : [0]).map(remoteView);
  const panesView =
    remoteViews.length === 2 && layout === "stacked" ? (
      <SplitView direction="row" sizes={splitOuter} onSizes={setSplitOuter}>
        {[
          localView,
          <SplitView key="remotes" direction="column" sizes={splitInner} onSizes={setSplitInner}>
            {remoteViews}
          </SplitView>,
        ]}
      </SplitView>
    ) : (
      <SplitView
        direction="row"
        sizes={remoteViews.length === 2 ? splitSide3 : splitSide2}
        onSizes={remoteViews.length === 2 ? setSplitSide3 : setSplitSide2}
      >
        {[localView, ...remoteViews]}
      </SplitView>
    );

  const replaceChoices =
    modal?.kind === "replace" ? (modal.only !== undefined ? [modal.only] : [0, 1]) : [];

  return (
    <main className="app">
      <Toolbar
        status={used.map((i) => {
          const s = slots[i];
          return s.state === "connecting" ? `${paneName(i)}…` : s.state === "failed" ? `${paneName(i)} ✗` : paneName(i);
        })}
        onQuickConnect={() => setModal({ kind: "quick" })}
        onManage={() => setModal({ kind: "manager", startWithImport: false })}
        layout={layout}
        onLayout={setLayout}
        panelOpen={panelOpen}
        onTogglePanel={togglePanel}
        onSettings={() => setModal({ kind: "settings" })}
      />
      <div className="workspace">
        <div className="panes">{panesView}</div>
        <BottomPanel
          log={log}
          transfers={transfers}
          edits={edits}
          hostColor={hostColor}
          onReconnect={handleConnect}
          sessionFor={sessionFor}
          tab={panelTab}
          onTab={(tab) => {
            if (tab === "log") log.markSeen();
            setPanelTab(tab);
          }}
          open={panelOpen}
          onToggle={togglePanel}
          height={panelHeight}
          onResize={setPanelHeight}
        />
      </div>
      {modal?.kind === "settings" && (
        <SettingsDialog onClose={() => setModal(null)} onAbout={() => setModal({ kind: "about" })} />
      )}
      {modal?.kind === "about" && <AboutDialog onClose={() => setModal(null)} />}
      {modal?.kind === "quick" && store && (
        <QuickConnect
          store={store}
          onConnect={handleConnect}
          onOpenBookmark={openBookmark}
          onManage={() => setModal({ kind: "manager", startWithImport: false })}
          onClose={() => setModal(null)}
        />
      )}
      {modal?.kind === "manager" && store && (
        <ConnectionManager
          store={store}
          save={connections.save}
          home={home}
          verbose={verboseHosts}
          onToggleVerbose={toggleVerbose}
          startWithImport={modal.startWithImport}
          onConnect={handleConnect}
          onClose={() => setModal(null)}
        />
      )}
      {modal?.kind === "replace" && (
        <ChooseDialog
          title={`Aprire ${modal.connection.name} al posto di…`}
          options={replaceChoices.map((i) => ({
            label: `Sostituisci ${paneName(i)}`,
            onSelect: () => guardClose(i, () => void openIn(i, modal.connection, modal.startPath)),
          }))}
          onCancel={() => setModal(null)}
        >
          <p>
            Sono già aperti due pannelli remoti. La connessione sostituita viene chiusa se nessun altro pannello la usa:
            i file aperti in modifica senza modifiche da caricare vengono chiusi, gli altri restano in "File aperti" e
            vengono caricati alla prossima connessione.
          </p>
        </ChooseDialog>
      )}
      {modal?.kind === "disconnect" && (
        <ConfirmDialog
          title={`Disconnettere ${paneName(modal.slot)}?`}
          confirmLabel="Disconnetti"
          onCancel={() => setModal(null)}
          onConfirm={() => {
            setModal(null);
            modal.then();
          }}
        >
          <p>Questi file aperti hanno modifiche non ancora caricate sul server:</p>
          <ul>
            {modal.files.map((file) => (
              <li key={file.id}>
                <code>{file.remotePath}</code>
              </li>
            ))}
          </ul>
          <p>
            Le copie locali non vanno perse: restano in "File aperti" e vengono caricate quando ti ricolleghi a questa
            connessione, con il controllo dei conflitti. Anche chiudendo l'app vengono ritrovate al riavvio.
          </p>
        </ConfirmDialog>
      )}
      <DragGhost />
      {prompts[0] && <AskpassDialog request={prompts[0]} onAnswer={answerPrompt} />}
      {connections.warning && (
        <div className="toast error-toast" role="alert">
          {connections.warning}
          <button className="inline" onClick={connections.dismissWarning}>
            ✕
          </button>
        </div>
      )}
      {appError && (
        <div className="toast error-toast" role="alert">
          {appError}
          <button className="inline" onClick={() => setAppError(null)}>
            ✕
          </button>
        </div>
      )}
      {!prompts[0] && edits.confirms[0] && (
        <EditConfirmDialog
          key={edits.confirms[0].id}
          confirm={edits.confirms[0]}
          protection={hostProtection(edits.confirms[0].host)}
          onAnswer={(allow, remember) => edits.answerConfirm(edits.confirms[0].id, allow, remember)}
        />
      )}
      {!prompts[0] && !edits.confirms[0] && edits.conflicts[0] && (
        <EditConflictDialog
          key={edits.conflicts[0].id}
          conflict={edits.conflicts[0]}
          onAnswer={(choice) => edits.resolve(edits.conflicts[0].id, choice)}
        />
      )}
      {!prompts[0] && !edits.confirms[0] && !edits.conflicts[0] && transfers.conflicts[0] && (
        <ConflictDialog
          key={transfers.conflicts[0].id}
          request={transfers.conflicts[0]}
          protection={transfers.conflicts[0].direction !== "download" ? hostProtection(transfers.conflicts[0].host) : null}
          onAnswer={transfers.answerConflict}
        />
      )}
    </main>
  );
}
