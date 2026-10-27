import type { ComponentProps } from "react";
import type { Category, ConnectResult, Connection } from "../api";
import { FilePane } from "./FilePane";

type Props = Omit<ComponentProps<typeof FilePane>, "title" | "actions" | "info" | "side" | "color"> & {
  connection: Connection;
  category: Category | undefined;
  session: ConnectResult;
  onDisconnect: () => void;
};

export function RemotePane({ connection, category, session, onDisconnect, ...paneProps }: Props) {
  const { capabilities } = session;
  return (
    <FilePane
      {...paneProps}
      side="remote"
      color={category?.color}
      title={
        <span className="remote-title" title={connection.name}>
          {connection.name}
          {category && (
            <span className="category-badge" style={{ background: category.color }}>
              {category.name}
            </span>
          )}
        </span>
      }
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
