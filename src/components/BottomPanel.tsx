import { useVirtualizer } from "@tanstack/react-virtual";
import { useLayoutEffect, useMemo, useRef, useState, type PointerEvent } from "react";
import { logOpenDir, type LogChannel, type LogEntry } from "../api";
import {
  MAX_LOG_CAPACITY,
  MIN_LOG_CAPACITY,
  emptyFilter,
  entryToText,
  filterEntries,
  formatDuration,
  formatLogTime,
  type LogFilter,
} from "../logStore";
import type { ActivityLog } from "../useActivityLog";
import type { Transfers } from "../useTransfers";
import { TransfersView } from "./TransfersView";
import type { Edits } from "../useEdits";
import { EditsView } from "./EditsView";

const ROW_HEIGHT = 22;
const CHANNELS: LogChannel[] = ["ssh", "sftp", "exec", "askpass", "local"];
const MIN_HEIGHT = 90;

export type BottomTab = "log" | "transfers" | "edits";

interface Props {
  log: ActivityLog;
  transfers: Transfers;
  edits: Edits;
  tab: BottomTab;
  onTab: (tab: BottomTab) => void;
  open: boolean;
  onToggle: () => void;
  height: number;
  onResize: (height: number) => void;
}

/** Bottom panel with the activity log and the transfer queue. */
export function BottomPanel({ log, transfers, edits, tab, onTab, open, onToggle, height, onResize }: Props) {
  const [filter, setFilter] = useState<LogFilter>(emptyFilter);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [showSettings, setShowSettings] = useState(false);
  const [copied, setCopied] = useState<string | null>(null);

  const visible = useMemo(() => filterEntries(log.entries, filter), [log.entries, filter]);
  const hosts = useMemo(
    () => [...new Set(log.entries.map((e) => e.host).filter((h): h is string => h !== null))].sort(),
    [log.entries],
  );
  const selected = log.entries.find((e) => e.id === selectedId) ?? null;

  const copy = (text: string, label: string) => {
    void navigator.clipboard.writeText(text).then(() => {
      setCopied(label);
      setTimeout(() => setCopied(null), 1200);
    });
  };

  const startResize = (e: PointerEvent<HTMLDivElement>) => {
    const startY = e.clientY;
    const startHeight = height;
    const max = window.innerHeight * 0.75;
    const move = (ev: globalThis.PointerEvent) =>
      onResize(Math.round(Math.min(max, Math.max(MIN_HEIGHT, startHeight + startY - ev.clientY))));
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  return (
    <section className={open ? "log-panel open" : "log-panel"} style={open ? { height } : undefined}>
      {open && <div className="log-resizer" onPointerDown={startResize} />}
      <header className="log-header">
        <button className="log-toggle" onClick={onToggle} title="Mostra/nascondi pannello (⌘J)">
          {open ? "▾" : "▸"}
        </button>
        <button
          className={tab === "log" ? "tab active" : "tab"}
          onClick={() => (open ? onTab("log") : (onTab("log"), onToggle()))}
        >
          Log
          {!open && log.unseenErrors > 0 && <span className="badge error-badge">{log.unseenErrors}</span>}
        </button>
        <button
          className={tab === "transfers" ? "tab active" : "tab"}
          onClick={() => (open ? onTab("transfers") : (onTab("transfers"), onToggle()))}
        >
          Trasferimenti
          {transfers.activeCount > 0 && <span className="badge active-badge">{transfers.activeCount}</span>}
        </button>
        <button
          className={tab === "edits" ? "tab active" : "tab"}
          onClick={() => (open ? onTab("edits") : (onTab("edits"), onToggle()))}
        >
          File aperti
          {edits.edits.length > 0 && (
            <span className={edits.unsyncedCount > 0 ? "badge error-badge" : "badge active-badge"}>{edits.edits.length}</span>
          )}
        </button>
        {open && tab === "transfers" && (
          <>
            <span className="spacer" />
            <button onClick={transfers.clearFinished} disabled={transfers.jobs.length === transfers.activeCount}>
              Rimuovi completati
            </button>
          </>
        )}
        {open && tab === "log" && (
          <>
            <select
              value={filter.errorsOnly ? "errors" : "all"}
              onChange={(e) => setFilter({ ...filter, errorsOnly: e.target.value === "errors" })}
            >
              <option value="all">Tutti</option>
              <option value="errors">Solo errori</option>
            </select>
            <select
              value={filter.host ?? ""}
              onChange={(e) => setFilter({ ...filter, host: e.target.value || null })}
            >
              <option value="">Tutti gli host</option>
              {hosts.map((h) => (
                <option key={h} value={h}>
                  {h}
                </option>
              ))}
            </select>
            <select
              value={filter.channel ?? ""}
              onChange={(e) => setFilter({ ...filter, channel: (e.target.value || null) as LogChannel | null })}
            >
              <option value="">Tutti i canali</option>
              {CHANNELS.map((c) => (
                <option key={c} value={c}>
                  {c.toUpperCase()}
                </option>
              ))}
            </select>
            <input
              className="log-search"
              type="search"
              placeholder="Filtra…"
              value={filter.text}
              onChange={(e) => setFilter({ ...filter, text: e.target.value })}
            />
            <span className="log-count">
              {visible.length === log.entries.length ? log.entries.length : `${visible.length}/${log.entries.length}`}
            </span>
            {copied && <span className="log-copied">Copiato: {copied}</span>}
            <span className="spacer" />
            <button title="Copia le righe visibili" onClick={() => copy(visible.map(entryToText).join("\n"), "log")}>
              ⎘
            </button>
            <button title="Svuota" onClick={() => { log.clear(); setSelectedId(null); }}>
              🗑
            </button>
            <div className="popover-anchor">
              <button title="Impostazioni log" onClick={() => setShowSettings(!showSettings)}>
                ⚙
              </button>
              {showSettings && <LogSettings log={log} onClose={() => setShowSettings(false)} />}
            </div>
          </>
        )}
      </header>
      {open && tab === "transfers" && <TransfersView transfers={transfers} />}
      {open && tab === "edits" && <EditsView edits={edits} />}
      {open && tab === "log" && (
        <div className={selected ? "log-body with-details" : "log-body"}>
          <LogList entries={visible} selectedId={selectedId} onSelect={setSelectedId} />
          {selected && (
            <LogDetails entry={selected} onCopy={copy} onClose={() => setSelectedId(null)} />
          )}
        </div>
      )}
    </section>
  );
}

function LogList(props: { entries: LogEntry[]; selectedId: number | null; onSelect: (id: number) => void }) {
  const { entries } = props;
  const scrollRef = useRef<HTMLDivElement>(null);
  const atBottom = useRef(true);
  const seenCount = useRef(entries.length);
  const [newCount, setNewCount] = useState(0);

  const virtualizer = useVirtualizer({
    count: entries.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 10,
  });

  // Follow new rows only while the user is at the bottom.
  useLayoutEffect(() => {
    const added = Math.max(0, entries.length - seenCount.current);
    seenCount.current = entries.length;
    if (entries.length === 0) {
      setNewCount(0);
    } else if (atBottom.current) {
      virtualizer.scrollToIndex(entries.length - 1, { align: "end" });
    } else if (added > 0) {
      setNewCount((n) => n + added);
    }
  }, [entries, virtualizer]);

  const onScroll = () => {
    const el = scrollRef.current;
    if (!el) return;
    atBottom.current = el.scrollTop + el.clientHeight >= el.scrollHeight - ROW_HEIGHT / 2;
    if (atBottom.current) setNewCount(0);
  };

  const jumpToEnd = () => {
    atBottom.current = true;
    setNewCount(0);
    virtualizer.scrollToIndex(entries.length - 1, { align: "end" });
  };

  return (
    <div className="log-list">
      <div className="log-scroll" ref={scrollRef} onScroll={onScroll}>
        <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
          {virtualizer.getVirtualItems().map((item) => {
            const e = entries[item.index];
            return (
              <div
                key={e.id}
                className={`log-row ${e.status}${e.id === props.selectedId ? " selected" : ""}`}
                style={{ transform: `translateY(${item.start}px)`, height: ROW_HEIGHT }}
                onClick={() => props.onSelect(e.id)}
              >
                <span className="log-time">{formatLogTime(e.time)}</span>
                <span className="log-host" title={e.host ?? ""}>{e.host ?? ""}</span>
                <span className={`log-channel ch-${e.channel}`}>{e.channel.toUpperCase()}</span>
                <span className="log-op">{e.op}</span>
                <span className="log-detail" title={e.detail}>{e.detail}</span>
                <span className="log-extra">{e.status === "error" ? e.error : e.extra}</span>
                <span className="log-duration">{formatDuration(e.durationMs)}</span>
                <span className="log-status">{e.status === "ok" ? "✓" : e.status === "error" ? "✗" : <span className="spinner" />}</span>
              </div>
            );
          })}
        </div>
      </div>
      {newCount > 0 && (
        <button className="log-new" onClick={jumpToEnd}>
          ↓ {newCount} {newCount === 1 ? "nuovo" : "nuovi"}
        </button>
      )}
    </div>
  );
}

function LogDetails(props: { entry: LogEntry; onCopy: (text: string, label: string) => void; onClose: () => void }) {
  const { entry: e, onCopy } = props;
  return (
    <aside className="log-details">
      <header>
        <strong>
          {e.channel.toUpperCase()} · {e.op}
        </strong>
        <span className="muted">
          {formatLogTime(e.time)} {e.host ? `· ${e.host}` : ""} {e.durationMs !== null ? `· ${formatDuration(e.durationMs)}` : ""}
        </span>
        <span className="spacer" />
        <button title="Copia riga" onClick={() => onCopy(entryToText(e), "riga")}>
          ⎘
        </button>
        <button title="Chiudi" onClick={props.onClose}>
          ✕
        </button>
      </header>
      <dl>
        <dt>Dettaglio</dt>
        <dd>
          <pre>{e.detail}</pre>
        </dd>
        {e.equivalent && (
          <>
            <dt>
              Comando terminale equivalente
              <button className="inline" onClick={() => onCopy(e.equivalent ?? "", "comando")}>
                Copia
              </button>
            </dt>
            <dd>
              <pre>{e.equivalent}</pre>
            </dd>
          </>
        )}
        {(e.extra || e.error) && (
          <>
            <dt>Esito</dt>
            <dd className={e.error ? "error-text" : undefined}>{e.error ?? e.extra}</dd>
          </>
        )}
        {e.output && (
          <>
            <dt>Output ssh</dt>
            <dd>
              <pre className="log-output">{e.output}</pre>
            </dd>
          </>
        )}
      </dl>
    </aside>
  );
}

function LogSettings(props: { log: ActivityLog; onClose: () => void }) {
  const [draft, setDraft] = useState(String(props.log.capacity));
  const apply = () => props.log.setCapacity(Number(draft));
  return (
    <div className="popover" onKeyDown={(e) => e.key === "Escape" && props.onClose()}>
      <label>
        Righe da conservare
        <input
          type="number"
          min={MIN_LOG_CAPACITY}
          max={MAX_LOG_CAPACITY}
          step={100}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={apply}
          onKeyDown={(e) => e.key === "Enter" && apply()}
        />
      </label>
      <p className="muted">
        Da {MIN_LOG_CAPACITY} a {MAX_LOG_CAPACITY}. Il log completo è anche salvato su file.
      </p>
      <button onClick={() => void logOpenDir()}>Apri cartella dei log</button>
    </div>
  );
}
