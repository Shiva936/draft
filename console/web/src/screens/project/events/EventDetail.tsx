import { Fragment, useState } from "react";
import { Icon } from "../../../icons";
import { Definitions, Tabs } from "../../../components/layout";
import { DetailDrawer } from "../../../components/DetailDrawer";
import { DataView } from "../../../components/DataView";
import { CandidateAvatar } from "../../../components/CandidateAvatar";
import { NONE, formatDateTime, humanize, relative, shortDigest } from "../../../lib/format";
import { eventTime, eventType, type CanonicalEvent } from "./Events";

/**
 * One canonical event, its metadata, and the hash-linked chain that produced
 * it. Fields Draft does not record (client address, user agent) are not shown.
 */
export function EventDetail({
  event,
  events,
  onClose,
}: {
  event: CanonicalEvent;
  events: CanonicalEvent[];
  onClose: () => void;
}) {
  const [tab, setTab] = useState("readable");
  const [copied, setCopied] = useState(false);
  const chain = auditChain(event, events);
  const correlation = typeof event.metadata?.correlation_id === "string" ? event.metadata.correlation_id : null;

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(event.event_id);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1500);
    } catch {
      /* Clipboard access can be denied; the id stays selectable on screen. */
    }
  };

  return (
    <DetailDrawer
      title={humanize(eventType(event))}
      eyebrow={<span className="mono">{eventType(event)}</span>}
      subtitle={`${formatDateTime(eventTime(event))} (${relative(eventTime(event))})`}
      onClose={onClose}
    >
      <div className="row-item" style={{ borderBottom: 0, padding: 0 }}>
        <code className="truncate">{event.event_id}</code>
        <button className="icon-button" onClick={copy} aria-label="Copy event ID">
          <Icon name={copied ? "check" : "copy"} size={16} />
        </button>
      </div>

      <Tabs
        label="Event representation"
        active={tab}
        onSelect={setTab}
        tabs={[
          { id: "readable", label: "Human-readable" },
          { id: "raw", label: "Raw" },
        ]}
      />

      {tab === "raw" ? (
        <DataView value={event} />
      ) : (
        <>
          <section className="stack tight">
            <h3>Overview</h3>
            <Definitions rows>
              <dt>Type</dt>
              <dd className="mono">{eventType(event)}</dd>
              <dt>Actor</dt>
              <dd>
                <span className="avatar-label">
                  <CandidateAvatar name={event.actor} />
                  <span>{event.actor}</span>
                </span>
              </dd>
              <dt>Subject</dt>
              <dd className="mono">{event.subject ?? NONE}</dd>
              <dt>Recorded</dt>
              <dd>{formatDateTime(eventTime(event))}</dd>
              <dt>Correlation</dt>
              <dd className="mono">{correlation ?? NONE}</dd>
            </Definitions>
          </section>

          <section className="stack tight">
            <h3>Integrity</h3>
            <Definitions rows>
              <dt>Record hash</dt>
              <dd className="mono">{shortDigest(event.record_hash)}</dd>
              <dt>Previous hash</dt>
              <dd className="mono">{shortDigest(event.previous_hash)}</dd>
            </Definitions>
          </section>

          {Object.keys(event.metadata ?? {}).length > 0 && (
            <section className="stack tight">
              <h3>Metadata</h3>
              <Definitions rows>
                {Object.entries(event.metadata).map(([key, value]) => (
                  <Fragment key={key}>
                    <dt>{humanize(key)}</dt>
                    <dd>{typeof value === "object" ? JSON.stringify(value) : String(value)}</dd>
                  </Fragment>
                ))}
              </Definitions>
            </section>
          )}

          <section className="stack tight">
            <h3>Audit chain</h3>
            {chain.length <= 1 ? (
              <p className="muted">This event is the first link recorded in the chain.</p>
            ) : (
              <ul className="audit-chain">
                {chain.map((link) => (
                  <li key={link.event_id}>
                    <Icon name="shield-check" size={16} className="success" />
                    <div>
                      <span className={link.event_id === event.event_id ? "mono" : undefined}>
                        {eventType(link)}
                      </span>
                      <small>
                        {link.actor} · {formatDateTime(eventTime(link))}
                      </small>
                    </div>
                  </li>
                ))}
              </ul>
            )}
          </section>
        </>
      )}
    </DetailDrawer>
  );
}

/** Walks `previous_hash` backwards to show how this event was reached. */
function auditChain(event: CanonicalEvent, events: CanonicalEvent[], depth = 6): CanonicalEvent[] {
  const byHash = new Map(events.map((entry) => [entry.record_hash, entry]));
  const chain: CanonicalEvent[] = [event];
  let current = event;
  while (chain.length < depth) {
    const previous = byHash.get(current.previous_hash);
    if (!previous || previous.event_id === current.event_id) break;
    chain.push(previous);
    current = previous;
  }
  return chain;
}
