import { useCallback, useRef, useState } from "react";
import { connect, disconnect, listDir, type ConnectResult, type Connection } from "./api";
import { usePane, type PaneState } from "./usePane";

/** At most two remote panes; `connection` is the stored connection as it was when opening. */
export type SlotState =
  | { state: "idle" }
  | { state: "connecting"; connection: Connection }
  | { state: "connected"; connection: Connection; result: ConnectResult; startPath: string }
  | { state: "failed"; connection: Connection; error: string };

type Slots = [SlotState, SlotState];

export interface RemoteSlots {
  slots: Slots;
  panes: [PaneState, PaneState];
  /**
   * Opens `connection` in `slot`, closing what was there. When the other slot
   * shows the same connection, its session is shared (one master, server-side
   * operations between the two panes). Resolves to null on failure.
   */
  open: (slot: number, connection: Connection, startPath?: string) => Promise<ConnectResult | null>;
  close: (slot: number) => Promise<void>;
}

const IDLE: SlotState = { state: "idle" };
const notConnected = () => Promise.reject(new Error("non connesso"));

export const sessionOf = (slot: SlotState): ConnectResult | null => (slot.state === "connected" ? slot.result : null);

/** `verbose(id)`: diagnostic mode; `onSessionClosed`: a session was disconnected (no pane uses it any more). */
export function useRemoteSlots(verbose: (connectionId: string) => boolean, onSessionClosed: (sessionId: number) => void): RemoteSlots {
  const [slots, setSlots] = useState<Slots>([IDLE, IDLE]);
  // Async operations read and write the latest slots, not the render's.
  const ref = useRef(slots);
  const update = (index: number, value: SlotState) => {
    const next: Slots = [...ref.current];
    next[index] = value;
    ref.current = next;
    setSlots(next);
  };

  const session0 = sessionOf(slots[0]);
  const session1 = sessionOf(slots[1]);
  const list0 = useCallback((path: string) => (session0 ? listDir(session0.sessionId, path) : notConnected()), [session0]);
  const list1 = useCallback((path: string) => (session1 ? listDir(session1.sessionId, path) : notConnected()), [session1]);
  const pane0 = usePane(list0, slots[0].state === "connected" ? slots[0].startPath : null);
  const pane1 = usePane(list1, slots[1].state === "connected" ? slots[1].startPath : null);

  const close = async (index: number) => {
    const slot = ref.current[index];
    update(index, IDLE);
    const other = sessionOf(ref.current[1 - index]);
    if (slot.state === "connected" && other?.sessionId !== slot.result.sessionId) {
      await disconnect(slot.result.sessionId);
      onSessionClosed(slot.result.sessionId);
    }
  };

  const open = async (index: number, connection: Connection, startPath?: string) => {
    await close(index);
    const other = ref.current[1 - index];
    if (other.state === "connected" && other.connection.id === connection.id) {
      const path = startPath ?? connection.remoteDir ?? other.result.home;
      update(index, { state: "connected", connection, result: other.result, startPath: path });
      return other.result;
    }
    const pending: SlotState = { state: "connecting", connection };
    update(index, pending);
    try {
      const result = await connect(connection.id, verbose(connection.id));
      if (ref.current[index] !== pending) {
        // Closed or replaced while connecting.
        void disconnect(result.sessionId);
        return null;
      }
      update(index, { state: "connected", connection, result, startPath: startPath ?? connection.remoteDir ?? result.home });
      return result;
    } catch (e) {
      if (ref.current[index] === pending) update(index, { state: "failed", connection, error: String(e) });
      return null;
    }
  };

  return { slots, panes: [pane0, pane1], open, close };
}
