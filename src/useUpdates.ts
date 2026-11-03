// Update checks and installation of the app (backend: src-tauri/src/update.rs).

import { useCallback, useEffect, useRef, useState } from "react";
import {
  onUpdateProgress,
  updateCheck,
  updateInstall,
  updateRestart,
  type UpdateInfo,
  type UpdateProgress,
} from "./api";
import { usePersistentState } from "./settings";
import { CHECK_INTERVAL_MS, FIRST_CHECK_DELAY_MS, shouldOffer } from "./updates";

export type UpdatePhase =
  /** Nothing to show. */
  | "idle"
  | "checking"
  /** A manual check found nothing newer. */
  | "upToDate"
  | "available"
  | "installing"
  /** Installed: runs after a restart. */
  | "installed"
  | "error";

export interface Updates {
  phase: UpdatePhase;
  info: UpdateInfo | null;
  progress: UpdateProgress | null;
  error: string | null;
  /** Check at start and once a day (localStorage `updates.auto`). */
  auto: boolean;
  setAuto: (auto: boolean) => void;
  /** Manual check ("Controlla ora", menu): shows the result even when there is nothing new. */
  check: () => void;
  install: () => void;
  restart: () => void;
  /** Automatic checks stop offering this version. */
  skip: () => void;
  /** Hides the result of a manual check or an error. */
  dismiss: () => void;
}

export function useUpdates(): Updates {
  const [auto, setAuto] = usePersistentState("updates.auto", true);
  const [skipped, setSkipped] = usePersistentState<string | null>("updates.skipped", null);
  const [phase, setPhase] = useState<UpdatePhase>("idle");
  const [info, setInfo] = useState<UpdateInfo | null>(null);
  const [progress, setProgress] = useState<UpdateProgress | null>(null);
  const [error, setError] = useState<string | null>(null);
  const phaseRef = useRef(phase);
  phaseRef.current = phase;

  const run = useCallback(
    (manual: boolean) => {
      // Never interrupt an installation, nor forget one waiting for a restart.
      if (["checking", "installing", "installed"].includes(phaseRef.current)) return;
      if (manual) setPhase("checking");
      setError(null);
      updateCheck().then(
        (found) => {
          if (found && shouldOffer(found.version, skipped, manual)) {
            setInfo(found);
            setPhase("available");
          } else {
            setInfo(null);
            setPhase(manual ? "upToDate" : "idle");
          }
        },
        (e) => {
          // Automatic checks fail quietly (offline, no release yet): the log has them.
          if (manual) {
            setError(String(e));
            setPhase("error");
          } else if (phaseRef.current === "checking") {
            setPhase("idle");
          }
        },
      );
    },
    [skipped],
  );

  const runRef = useRef(run);
  runRef.current = run;
  useEffect(() => {
    if (!auto) return;
    const first = setTimeout(() => runRef.current(false), FIRST_CHECK_DELAY_MS);
    const daily = setInterval(() => runRef.current(false), CHECK_INTERVAL_MS);
    return () => {
      clearTimeout(first);
      clearInterval(daily);
    };
  }, [auto]);

  useEffect(() => {
    const unlisten = onUpdateProgress(setProgress);
    return () => void unlisten.then((fn) => fn());
  }, []);

  return {
    phase,
    info,
    progress,
    error,
    auto,
    setAuto,
    check: () => run(true),
    install: () => {
      setPhase("installing");
      setProgress(null);
      setError(null);
      updateInstall().then(
        () => setPhase("installed"),
        (e) => {
          setError(String(e));
          setPhase("available");
        },
      );
    },
    restart: () => void updateRestart().catch((e) => setError(String(e))),
    skip: () => {
      if (info) setSkipped(info.version);
      setInfo(null);
      setPhase("idle");
    },
    dismiss: () => {
      if (phaseRef.current === "upToDate" || phaseRef.current === "error") setPhase("idle");
    },
  };
}
