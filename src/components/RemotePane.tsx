import type { ComponentProps } from "react";
import type { Category, ConnectResult, Connection } from "../api";
import { FilePane } from "./FilePane";

type Props = Omit<ComponentProps<typeof FilePane>, "title" | "actions" | "info" | "color"> & {
  connection: Connection;
  /** Shown name: the connection name, numbered when both panes show it. */
  name: string;
  category: Category | undefined;
  session: ConnectResult;
  onDisconnect: () => void;
  onOpenInOther: () => void;
};

export function RemotePane({ connection, name, category, session, onDisconnect, onOpenInOther, ...paneProps }: Props) {
  const { capabilities } = session;
  return (
    <FilePane
      {...paneProps}
      color={category?.color}
      title={
        <span className="remote-title" title={connection.name}>
          {name}
          {category && (
            <span className="category-badge" style={{ background: category.color }}>
              {category.name}
            </span>
          )}
        </span>
      }
      actions={
        <>
          <button title="Apri questa cartella anche nell'altro pannello (stessa connessione)" onClick={onOpenInOther}>
            ⧉
          </button>
          <button title="Chiudi il pannello" onClick={onDisconnect}>
            ✕
          </button>
        </>
      }
      info={
        capabilities.shell
          ? `Shell ${capabilities.os ?? ""} · ${capabilities.commands.join(", ")}`
          : "Account solo SFTP"
      }
    />
  );
}
