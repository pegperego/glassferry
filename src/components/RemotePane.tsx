import { useCallback } from "react";
import { listDir, type ConnectResult } from "../api";
import { usePane } from "../usePane";
import { FilePane } from "./FilePane";

interface Props {
  alias: string;
  connection: ConnectResult;
  active: boolean;
  onActivate: () => void;
  onDisconnect: () => void;
}

export function RemotePane({ alias, connection, active, onActivate, onDisconnect }: Props) {
  const list = useCallback((path: string) => listDir(connection.sessionId, path), [connection.sessionId]);
  const pane = usePane(list, connection.home);
  const { capabilities } = connection;

  return (
    <FilePane
      pane={pane}
      title={alias}
      active={active}
      onActivate={onActivate}
      actions={
        <button title="Disconnetti" onClick={onDisconnect}>
          ⏏
        </button>
      }
      info={
        capabilities.shell
          ? `Shell ${capabilities.os ?? ""} · ${capabilities.commands.join(", ")}`
          : "Account solo SFTP"
      }
    />
  );
}
