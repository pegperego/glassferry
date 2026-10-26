import type { ComponentProps } from "react";
import type { ConnectResult } from "../api";
import { FilePane } from "./FilePane";

type Props = Omit<ComponentProps<typeof FilePane>, "title" | "actions" | "info" | "side"> & {
  alias: string;
  connection: ConnectResult;
  onDisconnect: () => void;
};

export function RemotePane({ alias, connection, onDisconnect, ...paneProps }: Props) {
  const { capabilities } = connection;
  return (
    <FilePane
      {...paneProps}
      side="remote"
      title={alias}
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
