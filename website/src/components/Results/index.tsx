import {usePluginData} from '@docusaurus/useGlobalData';
import React from 'react';

/**
 * The results surface.
 *
 * Fully prerendered — no client-side state. At the current entrant count a
 * ranked list is legible without controls, and the numbers being in the HTML is
 * what makes them reachable to a reader with JavaScript off, to a search index,
 * and to anyone who views source to check we are not computing them in the
 * browser. Filtering arrives when the row count justifies it, and it will
 * hydrate this markup rather than replace it.
 */

type Metric = {
  value: number;
  unit: string;
  higher_is_better: boolean;
  n: number;
  spread: number;
};

type Row = {
  key: string;
  group: string;
  entrant: string;
  variant_id: string;
  version: string | null;
  commit: string | null;
  env_id: string;
  harness_version: number;
  dataset_version: string;
  ts_ms: number;
  flags: string[];
  superseded_by: {reason: string} | null;
  metrics: Record<string, Metric>;
};

type Entrant = {
  entrant: {
    id: string;
    name: string;
    status: string;
    runtime: string;
    licence: string;
    vendor: string;
    language?: string[];
  };
  display?: {short?: string};
  variants?: {id: string; label: string; approach: string; default?: boolean}[];
  planned?: {blockers?: string};
  guarantees?: {delivery?: string; interval_ms?: number};
};

type Env = {
  id: string;
  class: string;
  host?: {description?: string; cpu?: string; cores?: number};
};

type Data = {
  entrants: Entrant[];
  environments: Env[];
  rows: Row[];
  groups: {key: string; env_id: string; harness_version: number}[];
  counts: {files: number; lines: number; kept: number};
  generatedAt: string | null;
};

const EMPTY: Data = {
  entrants: [],
  environments: [],
  rows: [],
  groups: [],
  counts: {files: 0, lines: 0, kept: 0},
  generatedAt: null,
};

function useData(): Data {
  return (usePluginData('bench-data') as Data | undefined) ?? EMPTY;
}

function fmt(m: Metric): string {
  const v = m.value;
  if (m.unit === 'records/s') {
    if (v >= 1e6) return `${(v / 1e6).toFixed(2)}M/s`;
    if (v >= 1e3) return `${(v / 1e3).toFixed(0)}k/s`;
    return `${v.toFixed(0)}/s`;
  }
  if (m.unit === 'bytes') {
    if (v >= 1e9) return `${(v / 1e9).toFixed(2)} GB`;
    if (v >= 1e6) return `${(v / 1e6).toFixed(0)} MB`;
    return `${v.toFixed(0)} B`;
  }
  if (m.unit === 'us') return `${v.toFixed(3)} µs`;
  return `${v.toFixed(2)} ${m.unit}`;
}

/** The state the site is in before any measurement has been recorded. */
function NoResults({data}: {data: Data}) {
  const active = data.entrants.filter((e) => e.entrant.status === 'active');
  const planned = data.entrants.filter((e) => e.entrant.status === 'planned');
  return (
    <div className="bench-empty">
      <h3>No measurements published yet</h3>
      <p>
        The harness, the workload and the fairness contract are in place and the
        arms are built, but no run has been recorded against them. Rather than
        show a number that does not exist, this page says so.
      </p>
      <p className="bench-note">
        {active.length} system{active.length === 1 ? '' : 's'} implemented,{' '}
        {planned.length} planned. What each of them is waiting on is written down
        in <a href="/roadmap">the roadmap</a>, and how they will be measured is in{' '}
        <a href="/methodology">the methodology</a> — both of which are worth more
        scrutiny before there are results than after.
      </p>
    </div>
  );
}

function Roster({data}: {data: Data}) {
  return (
    <div className="bench-scroll">
      <table className="bench-roster">
        <thead>
          <tr>
            <th>System</th>
            <th>Status</th>
            <th>Runtime</th>
            <th>Licence</th>
            <th>Delivery</th>
            <th>Arms</th>
          </tr>
        </thead>
        <tbody>
          {data.entrants.map((e) => {
            const id = e.entrant.id;
            const ours = e.entrant.vendor === 'self';
            const variants = e.variants ?? [];
            return (
              <tr key={id}>
                <td>
                  <strong>{e.entrant.name}</strong>
                  {ours && (
                    <>
                      {' '}
                      {/* Rendered from `vendor = "self"` in the descriptor.
                          Nothing here branches on the literal id. */}
                      <span className="bench-pill bench-pill--vendor">
                        run by the vendor
                      </span>
                    </>
                  )}
                </td>
                <td>{e.entrant.status}</td>
                <td>{e.entrant.runtime}</td>
                <td>{e.entrant.licence}</td>
                <td>{e.guarantees?.delivery ?? '—'}</td>
                <td>
                  {variants.length ? (
                    variants.map((v) => (
                      <div key={v.id}>
                        <code>{v.id}</code>{' '}
                        {v.approach !== 'realistic' && (
                          <span className="bench-pill bench-pill--muted">
                            {v.approach}
                          </span>
                        )}
                      </div>
                    ))
                  ) : (
                    <span className="bench-note">not yet implemented</span>
                  )}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

/** One comparability group. Records from different groups never share an axis. */
function Group({rows, metric}: {rows: Row[]; metric: string}) {
  const usable = rows.filter((r) => r.metrics[metric] && !r.superseded_by);
  if (!usable.length) return null;

  const proto = usable[0].metrics[metric];
  const best = Math.max(...usable.map((r) => r.metrics[metric].value));
  const sorted = [...usable].sort((a, b) =>
    proto.higher_is_better
      ? b.metrics[metric].value - a.metrics[metric].value
      : a.metrics[metric].value - b.metrics[metric].value,
  );

  return (
    <div className="bench-scroll">
      <table className="bench-roster">
        <thead>
          <tr>
            <th>Arm</th>
            <th>Version</th>
            <th>{metric}</th>
            <th />
            <th>Measured</th>
          </tr>
        </thead>
        <tbody>
          {sorted.map((r) => {
            const m = r.metrics[metric];
            const pct = Math.max(2, (m.value / best) * 100);
            return (
              <tr key={r.key}>
                <td>
                  {r.entrant} <code>{r.variant_id}</code>
                </td>
                <td>{r.version ?? r.commit ?? '—'}</td>
                <td style={{whiteSpace: 'nowrap'}}>
                  {fmt(m)}
                  {m.n > 1 && (
                    <span className="bench-note">
                      {' '}
                      (n={m.n}, ±{(m.spread * 50).toFixed(1)}%)
                    </span>
                  )}
                </td>
                <td style={{width: '40%'}}>
                  {/* A CSS bar, not SVG: a real DOM row is selectable,
                      searchable and reflows, and this list is meant to grow. */}
                  <div
                    style={{
                      background: 'var(--bench-track)',
                      borderRadius: 3,
                      height: 10,
                    }}
                  >
                    <div
                      style={{
                        width: `${pct}%`,
                        background: 'var(--bench-bar)',
                        borderRadius: 3,
                        height: 10,
                      }}
                    />
                  </div>
                </td>
                <td className="bench-note">
                  {new Date(r.ts_ms).toISOString().slice(0, 10)}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

export default function Results(): React.JSX.Element {
  const data = useData();

  if (!data.rows.length) {
    return (
      <>
        <NoResults data={data} />
        <h2>The systems</h2>
        <Roster data={data} />
      </>
    );
  }

  const indicative = data.environments.filter((e) => e.class === 'indicative');

  return (
    <>
      {indicative.length > 0 && (
        <div className="bench-empty">
          <strong>These figures are indicative, not authoritative.</strong>{' '}
          <span className="bench-note">
            Rendered from the environment&rsquo;s declared class, so it will
            disappear on its own when an authoritative environment is added
            rather than having to be remembered.
          </span>
        </div>
      )}

      {data.groups.map((g) => {
        const rows = data.rows.filter((r) => r.group === g.key);
        return (
          <section key={g.key}>
            <h3>
              {g.env_id} <span className="bench-note">· harness v{g.harness_version}</span>
            </h3>
            {/* Groups are rendered separately rather than merged. Records that
                differ in harness, dataset, environment or infrastructure shape
                describe different experiments, and averaging them would be the
                single most misleading thing this page could do. */}
            <Group rows={rows} metric="rows_per_s" />
            <Group rows={rows} metric="cpu_us_per_row" />
            <Group rows={rows} metric="peak_anon_bytes" />
          </section>
        );
      })}

      {data.groups.length > 1 && (
        <p className="bench-note">
          The groups above are <strong>not comparable to each other</strong>: they
          differ in environment, measurement protocol, corpus or infrastructure
          shape. See <a href="/methodology">what invalidates a comparison</a>.
        </p>
      )}

      <h2>The systems</h2>
      <Roster data={data} />
    </>
  );
}
