import { useVirtualizer } from "@tanstack/react-virtual";
import { useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { formatSize } from "../format";
import { usePersistentState } from "../settings";
import { compileFilter, filterLines, highlight, MAX_LINES, type TailLine } from "../tailView";
import { useTail, type TailStarter } from "../useTail";

const ROW_HEIGHT = 18;

const METHOD: Record<string, string> = {
  tail: "tail -F",
  sftp: "SFTP, controllo ogni 0,5–3 s (account senza shell)",
};

interface Props {
  path: string;
  /** Where the file is (connection name, or "Locale"). */
  host: ReactNode;
  start: TailStarter;
  onClose: () => void;
}

/**
 * Follows a file (tail) in a blocking overlay: opening it starts the tail,
 * closing it stops it.
 */
export function TailDialog({ path, host, start, onClose }: Props) {
  const tail = useTail(start, path);
  const [filterText, setFilterText] = useState("");
  const [caseSensitive, setCaseSensitive] = usePersistentState("tail.caseSensitive", false);
  const [wrap, setWrap] = usePersistentState("tail.wrap", false);
  const [paused, setPaused] = useState<TailLine[] | null>(null);
  const [follow, setFollow] = useState(true);
  const [copied, setCopied] = useState(false);
  const scrollRef = useRef<HTMLDivElement>(null);
  const filterRef = useRef<HTMLInputElement>(null);

  // Esc closes even when the focus left the dialog (e.g. a button that disappeared).
  const closeRef = useRef(onClose);
  closeRef.current = onClose;
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        closeRef.current();
      } else if (e.metaKey && e.key.toLowerCase() === "f") {
        e.preventDefault();
        filterRef.current?.focus();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const filter = useMemo(() => compileFilter(filterText, caseSensitive), [filterText, caseSensitive]);
  const source = paused ?? tail.buffer.lines;
  const visible = useMemo(() => filterLines(source, filter.regex), [source, filter.regex]);
  const newWhilePaused = paused ? tail.buffer.lines.filter((l) => l.id > (paused[paused.length - 1]?.id ?? 0)).length : 0;

  const virtualizer = useVirtualizer({
    count: visible.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 20,
  });

  // Wrapped rows have different heights: measure them again when it changes.
  useEffect(() => virtualizer.measure(), [wrap, virtualizer]);

  // New lines keep the view at the bottom, unless the user scrolled up.
  useLayoutEffect(() => {
    if (follow && visible.length > 0) virtualizer.scrollToIndex(visible.length - 1, { align: "end" });
  }, [visible, follow, virtualizer]);

  const onScroll = () => {
    const el = scrollRef.current;
    if (!el) return;
    const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < ROW_HEIGHT;
    if (atBottom !== follow) setFollow(atBottom);
  };

  const toBottom = () => {
    setFollow(true);
    if (visible.length > 0) virtualizer.scrollToIndex(visible.length - 1, { align: "end" });
  };

  const copy = () =>
    void navigator.clipboard.writeText(visible.map((l) => l.text).join("\n")).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    });

  const status =
    tail.status === "starting"
      ? "Avvio…"
      : tail.status === "error"
        ? null
        : paused
          ? `In pausa${newWhilePaused > 0 ? ` · ${newWhilePaused} righe nuove` : ""}`
          : "In ascolto";

  return (
    <div className="modal-backdrop">
      <div className="modal tail">
        <div className="tail-head">
          <h2>
            Segui <span className="tail-host">{host}</span>
          </h2>
          <code className="tail-path" title={path}>
            {path}
          </code>
          <span className="spacer" />
          {tail.method && <span className="muted small">{METHOD[tail.method]}</span>}
          <button className="inline" title="Chiudi e ferma (Esc)" aria-label="Chiudi" onClick={onClose}>
            ✕
          </button>
        </div>
        <div className="tail-toolbar">
          <input
            ref={filterRef}
            className={`mono tail-filter${filter.error ? " invalid" : ""}`}
            autoFocus
            value={filterText}
            placeholder="Filtro (espressione regolare)  ⌘F"
            spellCheck={false}
            title={filter.error ?? "Mostra solo le righe che corrispondono"}
            onChange={(e) => setFilterText(e.target.value)}
          />
          <button
            className={caseSensitive ? "toggle on" : ""}
            title="Distingui maiuscole e minuscole"
            onClick={() => setCaseSensitive(!caseSensitive)}
          >
            Aa
          </button>
          <button className={wrap ? "toggle on" : ""} title="A capo automatico" onClick={() => setWrap(!wrap)}>
            ↩ A capo
          </button>
          <button
            className={paused ? "toggle on" : ""}
            title={paused ? "Mostra le righe arrivate nel frattempo" : "Ferma la vista (la lettura continua)"}
            onClick={() => {
              setPaused(paused ? null : tail.buffer.lines);
              if (paused) toBottom();
            }}
          >
            {paused ? "▶ Riprendi" : "❚❚ Pausa"}
          </button>
          <button title="Svuota la vista" onClick={tail.clear}>
            Svuota
          </button>
          <button title="Copia le righe mostrate" onClick={copy}>
            {copied ? "Copiato" : "Copia"}
          </button>
        </div>
        {filter.error && <p className="error-text small tail-filter-error">{filter.error}</p>}
        <div
          ref={scrollRef}
          className={`tail-lines${wrap ? " wrap" : ""}`}
          onScroll={onScroll}
          role="log"
          aria-live="off"
        >
          <div style={{ height: virtualizer.getTotalSize(), position: "relative", minWidth: "100%" }}>
            {virtualizer.getVirtualItems().map((item) => {
              const line = visible[item.index];
              return (
                <div
                  key={line.id}
                  data-index={item.index}
                  ref={wrap ? virtualizer.measureElement : undefined}
                  className={line.notice ? "tail-line notice" : "tail-line"}
                  style={{ transform: `translateY(${item.start}px)`, height: wrap ? undefined : ROW_HEIGHT }}
                >
                  {line.notice
                    ? line.text
                    : highlight(line.text, filter.regex).map((s, i) => (s.match ? <mark key={i}>{s.text}</mark> : s.text))}
                </div>
              );
            })}
          </div>
          {visible.length === 0 && tail.status !== "starting" && (
            <p className="muted small tail-empty">
              {filter.regex ? "Nessuna riga corrisponde al filtro." : "Il file è vuoto: le righe nuove compaiono qui."}
            </p>
          )}
        </div>
        <div className="tail-foot small">
          {status && <span className="muted">{status}</span>}
          {tail.status === "error" && (
            <>
              <span className="error-text">Interrotto: {tail.error}</span>
              <button className="inline link" onClick={tail.retry}>
                Riprova
              </button>
            </>
          )}
          <span className="spacer" />
          {filter.regex && (
            <span className="muted">
              {visible.filter((l) => !l.notice).length} di {source.filter((l) => !l.notice).length} righe
            </span>
          )}
          {tail.buffer.dropped > 0 && (
            <span className="muted" title={`Si tengono le ultime ${MAX_LINES.toLocaleString("it-IT")} righe`}>
              {tail.buffer.dropped.toLocaleString("it-IT")} righe più vecchie scartate
            </span>
          )}
          <span className="muted">letto fino a {formatSize(tail.offset)}</span>
          {!follow && (
            <button className="inline link" onClick={toBottom}>
              ↓ Vai in fondo
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
