import { getCurrentWebview } from "@tauri-apps/api/webview";
import { useCallback, useEffect, useRef, useState } from "react";
import {
  askpassRespond,
  connect,
  disconnect,
  fsDelete,
  fsMkdir,
  fsMove,
  fsRename,
  listDir,
  localHome,
  localListDir,
  localOpen,
  onAskpassRequest,
  type AskpassRequest,
  type ConnectResult,
  type Connection as StoredConnection,
  type Direction,
  type JobInfo,
  type Target,
} from "./api";
import { AskpassDialog } from "./components/AskpassDialog";
import { BottomPanel, type BottomTab } from "./components/BottomPanel";
import { ConflictDialog } from "./components/ConflictDialog";
import { ConnectionManager } from "./components/ConnectionManager";
import { EditConflictDialog } from "./components/EditConflictDialog";
import { FilePane, type PaneOps } from "./components/FilePane";
import { QuickConnect } from "./components/QuickConnect";
import { RemotePane } from "./components/RemotePane";
import { Toolbar } from "./components/Toolbar";
import { categoryOf } from "./connections";
import { groupByParent, type DragPayload, type Side } from "./dnd";
import { usePersistentState } from "./settings";
import { useActivityLog } from "./useActivityLog";
import { useConnections } from "./useConnections";
import { useEdits } from "./useEdits";
import { usePane } from "./usePane";
import { useTransfers } from "./useTransfers";
import "./App.css";

/** `connection` is the stored connection as it was when connecting. */
type Connection =
  | { state: "idle" }
  | { state: "connecting"; connection: StoredConnection }
  | { state: "connected"; connection: StoredConnection; result: ConnectResult }
  | { state: "failed"; connection: StoredConnection; error: string };

type Modal = { kind: "quick" } | { kind: "manager"; startWithImport: boolean } | null;

const notConnected = () => Promise.reject(new Error("non connesso"));

function paneOps(target: Target, permanentDelete: boolean, deleteHint: string): PaneOps {
  return {
    mkdir: (dir, name) => fsMkdir(target, dir, name),
    rename: (dir, from, to) => fsRename(target, dir, from, to),
    move: (srcDir, names, dstDir) => fsMove(target, srcDir, names, dstDir),
    remove: (dir, names) => fsDelete(target, dir, names),
    permanentDelete,
    deleteHint,
  };
}

const localOps = paneOps({ kind: "local" }, false, "Potrai recuperarli dal Cestino.");

export default function App() {
  const connections = useConnections();
  const store = connections.store;
  const [modal, setModal] = useState<Modal>(null);
  const [connection, setConnection] = useState<Connection>({ state: "idle" });
  const [prompts, setPrompts] = useState<AskpassRequest[]>([]);
  const [home, setHome] = useState<string | null>(null);
  const [activeSide, setActiveSide] = useState<Side>("local");
  const [panelOpen, setPanelOpen] = usePersistentState("log.open", true);
  const [panelHeight, setPanelHeight] = usePersistentState("log.height", 200);
  const [panelTab, setPanelTab] = usePersistentState<BottomTab>("panel.tab", "log");
  const [verboseList, setVerboseList] = usePersistentState<string[]>("connections.verbose", []);
  const verboseHosts = new Set(verboseList);
  const log = useActivityLog(panelOpen && panelTab === "log");

  const session = connection.state === "connected" ? connection.result : null;
  const remoteList = useCallback(
    (path: string) => (session ? listDir(session.sessionId, path) : notConnected()),
    [session],
  );
  const localPane = usePane(localListDir, home);
  const remoteStart = connection.state === "connected" ? (connection.connection.remoteDir ?? connection.result.home) : null;
  const remotePane = usePane(remoteList, remoteStart);

  // Refresh the destination pane when a transfer into its directory ends.
  const panesRef = useRef({ localPane, remotePane });
  panesRef.current = { localPane, remotePane };
  const transfers = useTransfers((job: JobInfo) => {
    const pane = job.direction === "download" ? panesRef.current.localPane : panesRef.current.remotePane;
    if (pane.path !== null && (job.dstDir === pane.path || job.dstDir.startsWith(`${pane.path.replace(/\/$/, "")}/`)))
      void pane.refresh();
  });

  const remoteOps = session
    ? paneOps(
        { kind: "remote", sessionId: session.sessionId },
        true,
        session.capabilities.commands.includes("rm")
          ? "L'eliminazione sul server è definitiva (rm -rf) e include tutto il contenuto delle cartelle."
          : "L'eliminazione sul server è definitiva e include tutto il contenuto delle cartelle (visita ricorsiva via SFTP).",
      )
    : null;

  const [appError, setAppError] = useState<string | null>(null);
  const edits = useEdits((message) => setAppError(message));

  const startTransfer = (direction: Direction, srcDir: string, names: string[], dstDir: string) => {
    if (!session || names.length === 0) return;
    void transfers.start(session.sessionId, direction, srcDir, names, dstDir);
    setPanelTab("transfers");
    if (!panelOpen) setPanelOpen(true);
  };

  const togglePanel = () => {
    if (!panelOpen) log.markSeen();
    setPanelOpen(!panelOpen);
  };

  const toggleVerbose = (id: string) =>
    setVerboseList(verboseHosts.has(id) ? verboseList.filter((a) => a !== id) : [...verboseList, id]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.metaKey && e.key.toLowerCase() === "j") {
        e.preventDefault();
        togglePanel();
      } else if (e.metaKey && e.key.toLowerCase() === "k" && !prompts[0]) {
        e.preventDefault();
        setModal({ kind: "quick" });
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

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

  // Files dropped from the Finder onto the remote pane are uploaded.
  const finderDropRef = useRef<(paths: string[], x: number, y: number) => void>(() => {});
  finderDropRef.current = (paths, x, y) => {
    const target = document.elementFromPoint(x, y)?.closest("[data-pane]");
    if (target?.getAttribute("data-pane") !== "remote" || remotePane.path === null) return;
    for (const [dir, names] of groupByParent(paths)) startTransfer("upload", dir, names, remotePane.path);
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

  const handleConnect = async (connectionId: string) => {
    const target = store?.connections.find((c) => c.id === connectionId);
    if (!target) return;
    setModal(null);
    if (connection.state === "connected") {
      await disconnect(connection.result.sessionId);
      edits.dropSession(connection.result.sessionId);
    }
    setConnection({ state: "connecting", connection: target });
    try {
      const result = await connect(connectionId, verboseHosts.has(connectionId));
      setConnection({ state: "connected", connection: target, result });
      setActiveSide("remote");
      if (target.localDir) void localPane.navigate(target.localDir);
    } catch (e) {
      setConnection({ state: "failed", connection: target, error: String(e) });
    }
  };

  const handleDisconnect = async () => {
    if (connection.state === "connected") {
      await disconnect(connection.result.sessionId);
      edits.dropSession(connection.result.sessionId);
    }
    setConnection({ state: "idle" });
    setActiveSide("local");
  };

  const onDrop = (payload: DragPayload, targetDir: string) =>
    startTransfer(payload.side === "local" ? "upload" : "download", payload.dir, payload.names, targetDir);

  const remoteName = connection.state === "idle" ? "" : connection.connection.name;
  const remoteCategory =
    store && connection.state !== "idle" ? categoryOf(store, connection.connection) : undefined;

  return (
    <main className="app">
      <Toolbar
        current={
          connection.state === "connecting"
            ? `Connessione a ${remoteName}…`
            : connection.state === "connected"
              ? `Connesso a ${remoteName}`
              : null
        }
        onQuickConnect={() => setModal({ kind: "quick" })}
        onManage={() => setModal({ kind: "manager", startWithImport: false })}
        panelOpen={panelOpen}
        onTogglePanel={togglePanel}
      />
      <div className="workspace">
        <div className="panes">
          <FilePane
            pane={localPane}
            side="local"
            ops={localOps}
            title="Locale"
            active={activeSide === "local"}
            onActivate={() => setActiveSide("local")}
            transfer={
              session && remotePane.path !== null && localPane.path !== null
                ? {
                    label: "Carica →",
                    title: `Carica su ${remoteName}:${remotePane.path}`,
                    run: (names) => startTransfer("upload", localPane.path!, names, remotePane.path!),
                  }
                : undefined
            }
            onDropFiles={session ? onDrop : undefined}
            openFile={{ label: "Apri con l'app predefinita", run: (path) => void localOpen(path).catch((e) => setAppError(String(e))) }}
          />
          {connection.state === "connected" && remoteOps ? (
            <RemotePane
              ops={remoteOps}
              connection={connection.connection}
              category={remoteCategory}
              session={connection.result}
              pane={remotePane}
              active={activeSide === "remote"}
              onActivate={() => setActiveSide("remote")}
              onDisconnect={() => void handleDisconnect()}
              transfer={
                remotePane.path !== null && localPane.path !== null
                  ? {
                      label: "← Scarica",
                      title: `Scarica in ${localPane.path}`,
                      run: (names) => startTransfer("download", remotePane.path!, names, localPane.path!),
                    }
                  : undefined
              }
              onDropFiles={onDrop}
              openFile={{
                label: "Modifica",
                run: (path) => {
                  void edits.open(connection.result.sessionId, path);
                  setPanelTab("edits");
                  if (!panelOpen) setPanelOpen(true);
                },
              }}
            />
          ) : (
            <section className="pane placeholder">
              {connection.state === "idle" && (
                <p>
                  <button className="primary" onClick={() => setModal({ kind: "quick" })}>
                    + Nuovo collegamento
                  </button>{" "}
                  <kbd>⌘K</kbd>
                </p>
              )}
              {connection.state === "connecting" && <p>Connessione a {remoteName}…</p>}
              {connection.state === "failed" && (
                <div className="error">
                  <strong>Connessione a {remoteName} fallita</strong>
                  <pre>{connection.error}</pre>
                </div>
              )}
            </section>
          )}
        </div>
        <BottomPanel
          log={log}
          transfers={transfers}
          edits={edits}
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
      {modal?.kind === "quick" && store && (
        <QuickConnect
          store={store}
          onConnect={(id) => void handleConnect(id)}
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
          onConnect={(id) => void handleConnect(id)}
          onClose={() => setModal(null)}
        />
      )}
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
      {!prompts[0] && edits.conflicts[0] && (
        <EditConflictDialog
          key={edits.conflicts[0].id}
          conflict={edits.conflicts[0]}
          onAnswer={(choice) => edits.resolve(edits.conflicts[0].id, choice)}
        />
      )}
      {!prompts[0] && !edits.conflicts[0] && transfers.conflicts[0] && (
        <ConflictDialog key={transfers.conflicts[0].id} request={transfers.conflicts[0]} onAnswer={transfers.answerConflict} />
      )}
    </main>
  );
}
