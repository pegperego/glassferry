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
  listHosts,
  localHome,
  localListDir,
  localOpen,
  onAskpassRequest,
  type AskpassRequest,
  type ConnectResult,
  type Direction,
  type HostEntry,
  type JobInfo,
  type Target,
} from "./api";
import { AskpassDialog } from "./components/AskpassDialog";
import { BottomPanel, type BottomTab } from "./components/BottomPanel";
import { ConflictDialog } from "./components/ConflictDialog";
import { EditConflictDialog } from "./components/EditConflictDialog";
import { FilePane, type PaneOps } from "./components/FilePane";
import { HostList } from "./components/HostList";
import { RemotePane } from "./components/RemotePane";
import { groupByParent, type DragPayload, type Side } from "./dnd";
import { usePersistentState } from "./settings";
import { useActivityLog } from "./useActivityLog";
import { useEdits } from "./useEdits";
import { usePane } from "./usePane";
import { useTransfers } from "./useTransfers";
import "./App.css";

type Connection =
  | { state: "idle" }
  | { state: "connecting"; alias: string }
  | { state: "connected"; alias: string; result: ConnectResult }
  | { state: "failed"; alias: string; error: string };

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
  const [hosts, setHosts] = useState<HostEntry[]>([]);
  const [hostsError, setHostsError] = useState<string | null>(null);
  const [connection, setConnection] = useState<Connection>({ state: "idle" });
  const [prompts, setPrompts] = useState<AskpassRequest[]>([]);
  const [home, setHome] = useState<string | null>(null);
  const [activeSide, setActiveSide] = useState<Side>("local");
  const [panelOpen, setPanelOpen] = usePersistentState("log.open", true);
  const [panelHeight, setPanelHeight] = usePersistentState("log.height", 200);
  const [panelTab, setPanelTab] = usePersistentState<BottomTab>("panel.tab", "log");
  const [verboseList, setVerboseList] = usePersistentState<string[]>("hosts.verbose", []);
  const verboseHosts = new Set(verboseList);
  const log = useActivityLog(panelOpen && panelTab === "log");

  const session = connection.state === "connected" ? connection.result : null;
  const remoteList = useCallback(
    (path: string) => (session ? listDir(session.sessionId, path) : notConnected()),
    [session],
  );
  const localPane = usePane(localListDir, home);
  const remotePane = usePane(remoteList, session?.home ?? null);

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

  const toggleVerbose = (alias: string) =>
    setVerboseList(verboseHosts.has(alias) ? verboseList.filter((a) => a !== alias) : [...verboseList, alias]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.metaKey && e.key.toLowerCase() === "j") {
        e.preventDefault();
        togglePanel();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  useEffect(() => {
    listHosts().then(setHosts, (e) => setHostsError(String(e)));
    localHome().then(setHome, () => setHome("/"));
  }, []);

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

  const answerPrompt = (value: string | null) => {
    const [current, ...rest] = prompts;
    if (!current) return;
    setPrompts(rest);
    if (current.kind !== "notice") void askpassRespond(current.id, value);
  };

  const handleConnect = async (alias: string) => {
    if (connection.state === "connected") {
      await disconnect(connection.result.sessionId);
      edits.dropSession(connection.result.sessionId);
    }
    setConnection({ state: "connecting", alias });
    try {
      const result = await connect(alias, verboseHosts.has(alias));
      setConnection({ state: "connected", alias, result });
      setActiveSide("remote");
    } catch (e) {
      setConnection({ state: "failed", alias, error: String(e) });
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

  const activeAlias = connection.state === "idle" ? null : connection.alias;

  return (
    <main className="app">
      <HostList
        hosts={hosts}
        error={hostsError}
        activeAlias={activeAlias}
        busy={connection.state === "connecting"}
        verboseHosts={verboseHosts}
        onToggleVerbose={toggleVerbose}
        onConnect={(alias) => void handleConnect(alias)}
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
                    title: `Carica su ${connection.state === "connected" ? connection.alias : ""}:${remotePane.path}`,
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
              alias={connection.alias}
              connection={connection.result}
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
              {connection.state === "idle" && <p>Doppio clic su un host per connetterti.</p>}
              {connection.state === "connecting" && <p>Connessione a {connection.alias}…</p>}
              {connection.state === "failed" && (
                <div className="error">
                  <strong>Connessione a {connection.alias} fallita</strong>
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
      {prompts[0] && <AskpassDialog request={prompts[0]} onAnswer={answerPrompt} />}
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
