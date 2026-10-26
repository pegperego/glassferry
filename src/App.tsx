import { useEffect, useState } from "react";
import {
  askpassRespond,
  connect,
  disconnect,
  listHosts,
  localHome,
  localListDir,
  onAskpassRequest,
  type AskpassRequest,
  type ConnectResult,
  type HostEntry,
} from "./api";
import { AskpassDialog } from "./components/AskpassDialog";
import { FilePane } from "./components/FilePane";
import { HostList } from "./components/HostList";
import { RemotePane } from "./components/RemotePane";
import { usePane } from "./usePane";
import "./App.css";

type Connection =
  | { state: "idle" }
  | { state: "connecting"; alias: string }
  | { state: "connected"; alias: string; result: ConnectResult }
  | { state: "failed"; alias: string; error: string };

type Side = "local" | "remote";

export default function App() {
  const [hosts, setHosts] = useState<HostEntry[]>([]);
  const [hostsError, setHostsError] = useState<string | null>(null);
  const [connection, setConnection] = useState<Connection>({ state: "idle" });
  const [prompts, setPrompts] = useState<AskpassRequest[]>([]);
  const [home, setHome] = useState<string | null>(null);
  const [activeSide, setActiveSide] = useState<Side>("local");
  const localPane = usePane(localListDir, home);

  useEffect(() => {
    listHosts().then(setHosts, (e) => setHostsError(String(e)));
    localHome().then(setHome, () => setHome("/"));
  }, []);

  useEffect(() => {
    const unlisten = onAskpassRequest((request) => setPrompts((queue) => [...queue, request]));
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
    if (connection.state === "connected") await disconnect(connection.result.sessionId);
    setConnection({ state: "connecting", alias });
    try {
      const result = await connect(alias);
      setConnection({ state: "connected", alias, result });
      setActiveSide("remote");
    } catch (e) {
      setConnection({ state: "failed", alias, error: String(e) });
    }
  };

  const handleDisconnect = async () => {
    if (connection.state === "connected") await disconnect(connection.result.sessionId);
    setConnection({ state: "idle" });
    setActiveSide("local");
  };

  const activeAlias = connection.state === "idle" ? null : connection.alias;

  return (
    <main className="app">
      <HostList
        hosts={hosts}
        error={hostsError}
        activeAlias={activeAlias}
        busy={connection.state === "connecting"}
        onConnect={(alias) => void handleConnect(alias)}
      />
      <div className="panes">
        <FilePane
          pane={localPane}
          title="Locale"
          active={activeSide === "local"}
          onActivate={() => setActiveSide("local")}
        />
        {connection.state === "connected" ? (
          <RemotePane
            key={connection.result.sessionId}
            alias={connection.alias}
            connection={connection.result}
            active={activeSide === "remote"}
            onActivate={() => setActiveSide("remote")}
            onDisconnect={() => void handleDisconnect()}
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
      {prompts[0] && <AskpassDialog request={prompts[0]} onAnswer={answerPrompt} />}
    </main>
  );
}
