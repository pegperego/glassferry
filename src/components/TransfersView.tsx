import { useState } from "react";
import { isFinished, type JobInfo } from "../api";
import { formatSize } from "../format";
import { formatDuration } from "../logStore";
import type { Transfers } from "../useTransfers";

const STATE_LABEL: Record<JobInfo["state"], string> = {
  scanning: "Analisi…",
  running: "In corso",
  waitingConflict: "In attesa di conferma",
  done: "Completato",
  failed: "Completato con errori",
  cancelled: "Annullato",
};

interface Props {
  transfers: Transfers;
  /** Show only this host's jobs. */
  host: string | null;
  hostColor: (host: string) => string | undefined;
}

export function TransfersView({ transfers, host, hostColor }: Props) {
  const jobs = host ? transfers.jobs.filter((j) => j.host === host || j.dstHost === host) : transfers.jobs;
  if (transfers.jobs.length === 0) {
    return (
      <p className="transfers-empty muted">
        Nessun trasferimento. Seleziona dei file e premi F5, usa i pulsanti Carica/Scarica o trascinali nell'altro
        pannello.
      </p>
    );
  }
  return (
    <div className="transfers">
      {[...jobs].reverse().map((job) => (
        <TransferRow
          key={job.id}
          job={job}
          color={hostColor(job.host)}
          dstColor={job.dstHost ? hostColor(job.dstHost) : undefined}
          onCancel={() => transfers.cancel(job.id)}
        />
      ))}
    </div>
  );
}

const DIRECTION: Record<JobInfo["direction"], { icon: string; title: string }> = {
  upload: { icon: "↑", title: "Upload" },
  download: { icon: "↓", title: "Download" },
  remote: { icon: "⇄", title: "Copia tra pannelli remoti" },
};

interface RowProps {
  job: JobInfo;
  color: string | undefined;
  dstColor: string | undefined;
  onCancel: () => void;
}

function TransferRow({ job, color, dstColor, onCancel }: RowProps) {
  const [showErrors, setShowErrors] = useState(false);
  const finished = isFinished(job);
  const percent = job.totalBytes > 0 ? Math.min(100, (job.doneBytes / job.totalBytes) * 100) : finished ? 100 : 0;
  const remaining = job.bytesPerSec > 0 ? ((job.totalBytes - job.doneBytes) / job.bytesPerSec) * 1000 : null;
  const elapsed = (job.finishedAt ?? Date.now()) - job.startedAt;
  const what = job.names.length === 1 ? job.names[0] : `${job.names.length} elementi`;

  return (
    <div className={`transfer ${job.state}`}>
      <span className="transfer-dir" title={DIRECTION[job.direction].title}>
        {DIRECTION[job.direction].icon}
      </span>
      <div className="transfer-main">
        <div className="transfer-title">
          <strong>{what}</strong>
          <span className="host-chip">
            <span className="category-dot" style={{ background: color ?? "var(--muted)" }} />
            {job.host}
          </span>
          {job.dstHost && (
            <span className="host-chip">
              →
              <span className="category-dot" style={{ background: dstColor ?? "var(--muted)" }} />
              {job.dstHost}
            </span>
          )}
          <span className="muted" title={`${job.srcDir} → ${job.dstDir}`}>
            {job.direction === "download" ? `${job.srcDir} → ${job.dstDir} (locale)` : `${job.srcDir} → ${job.dstDir}`}
          </span>
        </div>
        <div className="progress">
          <div className="progress-bar" style={{ width: `${percent}%` }} />
        </div>
        <div className="transfer-meta muted">
          <span>{STATE_LABEL[job.state]}</span>
          <span>
            {formatSize(job.doneBytes)} di {formatSize(job.totalBytes)}
          </span>
          <span>
            {job.doneFiles}/{job.totalFiles} file
            {job.skippedFiles > 0 && `, ${job.skippedFiles} saltati`}
          </span>
          {!finished && job.bytesPerSec > 0 && <span>{formatSize(job.bytesPerSec)}/s</span>}
          {!finished && remaining !== null && <span>~{formatDuration(Math.round(remaining))} rimanenti</span>}
          {finished && <span>in {formatDuration(elapsed)}</span>}
          {!finished && job.current && (
            <span className="transfer-current" title={job.current}>
              {job.current}
            </span>
          )}
          {job.errors.length > 0 && (
            <button className="inline error-text" onClick={() => setShowErrors(!showErrors)}>
              {errorCount(job)} {errorCount(job) === 1 ? "errore" : "errori"} {showErrors ? "▴" : "▾"}
            </button>
          )}
        </div>
        {showErrors && (
          <ul className="transfer-errors">
            {job.errors.map((e, i) => (
              <li key={i}>{e}</li>
            ))}
          </ul>
        )}
      </div>
      {!finished && (
        <button title="Annulla" onClick={onCancel}>
          ✕
        </button>
      )}
    </div>
  );
}

const errorCount = (job: JobInfo) => job.failedFiles || job.errors.length;
