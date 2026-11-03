// One followed file: starts the tail, collects its events, stops it on unmount
// (closing the dialog stops the tail: none is left running).

import { useCallback, useEffect, useRef, useState } from "react";
import { onTailEvent, tailStop, type TailMethod } from "./api";
import { append, emptyBuffer, INITIAL_LINES, type TailBuffer } from "./tailView";

export type TailStatus = "starting" | "running" | "error";

export type TailStarter = (tailId: number, path: string, lines: number, fromOffset: number | null) => Promise<{ method: TailMethod }>;

let nextTailId = 1;

export interface Tail {
  buffer: TailBuffer;
  method: TailMethod | null;
  status: TailStatus;
  error: string | null;
  /** File position read so far. */
  offset: number;
  /** After an error: continues from `offset`, without repeating lines. */
  retry: () => void;
  clear: () => void;
}

export function useTail(start: TailStarter, path: string): Tail {
  const [buffer, setBuffer] = useState<TailBuffer>(emptyBuffer);
  const [method, setMethod] = useState<TailMethod | null>(null);
  const [status, setStatus] = useState<TailStatus>("starting");
  const [error, setError] = useState<string | null>(null);
  const [offset, setOffset] = useState(0);
  const current = useRef<number | null>(null);
  const offsetRef = useRef(0);
  const startRef = useRef(start);
  startRef.current = start;

  const run = useCallback(
    (fromOffset: number | null) => {
      const id = nextTailId++;
      current.current = id;
      setStatus("starting");
      setError(null);
      startRef.current(id, path, INITIAL_LINES, fromOffset).then(
        (started) => {
          if (current.current !== id) return;
          setMethod(started.method);
          setStatus("running");
        },
        (e) => {
          if (current.current !== id) return;
          setError(String(e));
          setStatus("error");
        },
      );
    },
    [path],
  );

  useEffect(() => {
    let cancelled = false;
    const listening = onTailEvent((event) => {
      // A listener being removed (unlisten is asynchronous) must not add lines twice.
      if (cancelled || event.tailId !== current.current) return;
      if (event.kind === "lines") {
        setBuffer((b) => append(b, event.lines));
        offsetRef.current = event.offset;
        setOffset(event.offset);
      } else if (event.kind === "notice") {
        setBuffer((b) => append(b, [event.message], true));
      } else {
        offsetRef.current = event.offset;
        setOffset(event.offset);
        current.current = null;
        if (event.error) {
          setError(event.error);
          setStatus("error");
        }
      }
    });
    // Start only once the listener is in place: no early lines lost.
    void listening.then(() => {
      if (!cancelled) run(null);
    });
    return () => {
      cancelled = true;
      if (current.current !== null) void tailStop(current.current).catch(() => {});
      current.current = null;
      void listening.then((fn) => fn());
    };
  }, [run]);

  return {
    buffer,
    method,
    status,
    error,
    offset,
    retry: () => {
      setBuffer((b) => append(b, ["— ripresa —"], true));
      run(offsetRef.current);
    },
    clear: () => setBuffer((b) => ({ ...b, lines: [] })),
  };
}
