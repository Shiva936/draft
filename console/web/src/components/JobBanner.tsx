import { Icon } from "../icons";
import { cancelJob } from "../api";
import type { ServiceJob } from "../contracts";
import { humanize } from "../lib/format";

/** Progress and cancellation for a durable daemon job. */
export function JobBanner({ job }: { job: ServiceJob | null }) {
  if (!job) return null;
  const total = job.progress_total ?? 0;
  return (
    <div className="banner running" role="status">
      <Icon name="refresh" size={18} />
      <div>
        <strong>
          {humanize(job.kind)} · {humanize(job.phase)}
        </strong>
        <p>
          Durable job <code>{job.id}</code>
          {total > 0 && ` · ${job.progress_completed}/${total}`}
        </p>
      </div>
      <button
        className="button"
        disabled={job.cancellation_requested}
        onClick={() => void cancelJob(job.id)}
      >
        {job.cancellation_requested ? "Cancelling…" : "Cancel"}
      </button>
    </div>
  );
}
