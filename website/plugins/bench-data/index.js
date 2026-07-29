// Reads the benchmark's committed data at build time and hands it to pages.
//
// Three sources, all outside the site root, none of them importable by webpack:
// the entrant descriptors (TOML), the environment profiles (TOML), and the
// results (JSONL — an unknown module type). So they are read here and published
// through `usePluginData`.
//
// WHAT GOES INTO GLOBAL DATA, AND WHY IT IS BOUNDED
//
// Docusaurus global data is not code-split per route: whatever is put here ships
// in the main bundle to every visitor, including one who only opens the
// methodology page. The framework repository's equivalent plugin publishes every
// record, which at 706 records is ~975 KB of global data and ~227 KB gzipped
// inside main.js, growing linearly.
//
// So this publishes a CATALOGUE — sized by the number of entrants, environments
// and comparability groups, not by the number of records — plus a pre-aggregated
// summary row per arm. The full per-record archive is deliberately not here; when
// the archive justifies it, it becomes content-hashed shards under static/ that
// are fetched only when a reader expands history.
//
// COMPARABILITY IS ENFORCED HERE, NOT IN THE COMPONENTS
//
// Records that differ in (harness_version, dataset_version, env_id, infra digest)
// describe different experiments and must never be averaged together. Grouping
// them at load time means a component cannot accidentally mix them, and the
// "these are not comparable" case becomes data the page can render rather than a
// mistake nobody notices.

const fs = require('node:fs');
const path = require('node:path');

const TOML = require('smol-toml');

const PLUGIN = 'bench-data';

/** Repository root, relative to `website/`. */
function repoRoot(siteDir) {
  return process.env.BENCH_ROOT || path.resolve(siteDir, '..');
}

/**
 * Reads one descriptor or environment profile.
 *
 * A real TOML parser, not the hand-rolled subset that used to live here. This is
 * the seam where the site has to agree with the harness about what a descriptor
 * says, and the two were parsing the same files with different implementations:
 * the Rust side uses the `toml` crate with `deny_unknown_fields`, while this side
 * mis-read an inline `key = "x" # note` as the value `x" # note`, reattached a
 * nested `[a.b]` under an open `[[array]]` to the root, and carried a
 * write-only `arrayMode` flag that betrayed the confusion. Anything mis-parsed
 * here is rendered beside a published number, and `entrants_are_valid` cannot
 * catch it because that test validates the other parser.
 *
 * `smol-toml` is dependency-free and build-time only, so the supply-chain
 * argument the old comment made against a real parser does not apply: it never
 * reaches a visitor's browser.
 *
 * A file that does not parse throws rather than yielding a partial object. A
 * silently half-read descriptor is how a system ends up on the page with its
 * guarantees missing.
 */
function readToml(file) {
  try {
    return TOML.parse(fs.readFileSync(file, 'utf8'));
  } catch (e) {
    throw new Error(`${file}: ${e.message}`);
  }
}

function readDirSafe(dir) {
  try {
    return fs.readdirSync(dir, { withFileTypes: true });
  } catch {
    return [];
  }
}

function loadEntrants(root) {
  const dir = path.join(root, 'entrants');
  return readDirSafe(dir)
    .filter((e) => e.isDirectory())
    .map((e) => path.join(dir, e.name, 'entrant.toml'))
    .filter((p) => fs.existsSync(p))
    .map((p) => {
      const spec = readToml(p);
      if (!spec.entrant || !spec.entrant.id) {
        throw new Error(`${p}: no [entrant].id — every system must say what it is`);
      }
      return spec;
    })
    .sort((a, b) => (a.display?.order ?? 0) - (b.display?.order ?? 0));
}

function loadEnvironments(root) {
  const dir = path.join(root, 'environments');
  return readDirSafe(dir)
    .filter((e) => e.isFile() && e.name.endsWith('.toml'))
    .map((e) => {
      const p = path.join(dir, e.name);
      const spec = readToml(p);
      if (!spec.id) throw new Error(`${p}: no id — an environment is the unit of comparability`);
      return spec;
    });
}

function walkJsonl(dir, out) {
  for (const e of readDirSafe(dir)) {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) walkJsonl(p, out);
    else if (e.name.endsWith('.jsonl')) out.push(p);
  }
  return out;
}

function loadRecords(root) {
  const files = walkJsonl(path.join(root, 'results'), []).sort();
  const records = [];
  const counts = { files: files.length, lines: 0, kept: 0, skippedSchema: 0, skippedParse: 0 };
  for (const f of files) {
    for (const line of fs.readFileSync(f, 'utf8').split('\n')) {
      if (!line.trim()) continue;
      counts.lines += 1;
      let rec;
      try {
        rec = JSON.parse(line);
      } catch {
        counts.skippedParse += 1;
        continue;
      }
      // Schema 2 only. A v1 record has no system under test, no environment and
      // no comparability fields; rendering one would mean inventing all three.
      if (!rec || typeof rec !== 'object' || rec.schema !== 2) {
        counts.skippedSchema += 1;
        continue;
      }
      counts.kept += 1;
      records.push(rec);
    }
  }
  return { records, counts };
}

/**
 * The key that decides what may share an axis.
 *
 * The first four components are provenance: two records that differ in any of
 * them describe different experiments, and methodology/ makes three of them
 * hard splits.
 *
 * `mode` is here for a different reason and it is not optional. `rows_per_s`
 * means "how fast can this go" in drain and "the rate we asked for" in
 * sustained, so two arms of wildly different capacity report the same number;
 * the efficiency figures were taken with the broker serving writes and reads at
 * once and a generator competing for cores, which is the whole argument drain
 * exists for; and latency is single-mode by construction. A row is not an axis,
 * and it is the axis that misleads.
 */
function groupKey(rec) {
  return [
    rec.run?.env_id,
    rec.run?.harness_version,
    rec.run?.dataset_version,
    rec.run?.infra?.digest,
    `mode-${rec.variant?.mode ?? '?'}`,
  ].join('|');
}

/**
 * A stable fingerprint of an arm's configuration.
 *
 * Every knob the driver recorded, in sorted order. Two records that differ here
 * were not the same experiment and must not be medianed together, however close
 * in time they were: `--batches 150000` and `--batches 1500000` are a tenth of
 * the corpus apart and produce entirely different cache behaviour.
 */
function variantKey(rec) {
  const v = rec.variant ?? {};
  return JSON.stringify(v, Object.keys(v).sort());
}

function median(xs) {
  if (!xs.length) return null;
  const s = [...xs].sort((a, b) => a - b);
  const m = Math.floor(s.length / 2);
  return s.length % 2 ? s[m] : (s[m - 1] + s[m]) / 2;
}

/** Statuses that carry publishable numbers. Mirrors `Status::carries_metrics`. */
const CARRIES_METRICS = new Set(['ok', 'infra_bound']);

/**
 * Severity order for aggregating a repetition's status up to its row.
 *
 * A row takes the WORST status among the repetitions behind it, never the newest
 * one. An arm that crossed the 70% headroom limit on one repetition of three
 * demonstrably crossed it, and the aggregate of those three cannot be published
 * as a system comparison just because the last repetition happened to come in
 * under.
 */
const STATUS_SEVERITY = {ok: 0, infra_bound: 1};

/**
 * Severity of a status this build does not recognise.
 *
 * Above every known value, so an unknown status always wins and the row is never
 * ranked. Scoring it 0 — which this did — made it tie with `ok` and lose, so a
 * status added by a newer harness would have been silently published as sound by
 * an older site. That is the same fail-open mistake `approach` used to make, and
 * it fails in the same direction: towards publishing something we cannot vouch
 * for.
 */
const UNKNOWN_SEVERITY = Number.MAX_SAFE_INTEGER;

const severity = (status) => STATUS_SEVERITY[status] ?? UNKNOWN_SEVERITY;

function worstStatus(recs) {
  return recs.map((r) => r.status).reduce((a, b) => (severity(b) > severity(a) ? b : a), 'ok');
}

/**
 * One summary row per (group, entrant, variant, configuration, version).
 *
 * Repetitions within a single invocation are aggregated by median. Runs from
 * DIFFERENT sittings are not: they get their own rows. That is the correction to
 * the framework site's aggregator, which hashes only variant keys and so
 * silently medians a re-run months later into the original figure while
 * captioning it with the newest date.
 *
 * Three properties this function is responsible for, each of which it previously
 * got wrong:
 *
 * - **`infra_bound` records are kept.** They were dropped here by a
 *   `status !== 'ok'` filter, which made "we ran it and it blew the headroom
 *   limit" render identically to "we never ran it" — the exact distinction
 *   `Status::InfraBound` exists to preserve. They are kept, carried through with
 *   their status, and the page refuses to rank them.
 * - **Configuration is part of the identity.** The key omitted `variant`
 *   entirely, so two sweeps of the same arm at different knob settings on the
 *   same day were medianed into one number captioned as run-to-run spread.
 *
 * Known remaining limitation: a "sitting" is still approximated by UTC calendar
 * day, so a sweep straddling midnight splits into two rows and two sweeps of an
 * identical configuration on one day merge. Fixing it properly needs an
 * invocation id on the record, which is a harness change rather than a site one.
 */
function summarise(records) {
  const byKey = new Map();
  const attempts = [];
  for (const rec of records) {
    if (!CARRIES_METRICS.has(rec.status)) {
      // Attempted and produced no publishable number. Surfaced as an explicit
      // gap rather than an absence a reader would read as "not tried".
      attempts.push({
        group: groupKey(rec),
        entrant: rec.sut?.entrant,
        variant_id: rec.sut?.variant_id,
        status: rec.status,
        note: rec.note ?? null,
        ts_ms: rec.run?.ts_ms ?? 0,
      });
      continue;
    }
    const key = [
      groupKey(rec),
      rec.sut?.entrant,
      rec.sut?.variant_id,
      rec.sut?.version ?? rec.sut?.commit ?? '?',
      variantKey(rec),
      // Distinct sittings stay distinct. Without this, a re-run silently joins
      // the original and the archive stops being a history.
      //
      // `invocation_id` is minted once per `bench run` from harness 2 on, so a
      // sitting is now identified exactly rather than approximated. The calendar
      // day remains the fallback for records written before the field existed,
      // and it is only an approximation: a sweep crossing midnight UTC split
      // into two published rows, and two sweeps on one day merged into one.
      rec.run?.invocation_id || new Date(rec.run?.ts_ms ?? 0).toISOString().slice(0, 10),
    ].join('|');
    if (!byKey.has(key)) byKey.set(key, []);
    byKey.get(key).push(rec);
  }

  const rows = [];
  for (const [key, reps] of byKey) {
    const newest = reps.reduce((a, b) => (a.run.ts_ms >= b.run.ts_ms ? a : b));

    const counted = reps;

    const metrics = {};
    const names = new Set(counted.flatMap((r) => Object.keys(r.metrics || {})));
    for (const name of names) {
      const vals = counted.map((r) => r.metrics?.[name]?.value).filter((v) => typeof v === 'number');
      if (!vals.length) continue;
      const proto = counted.find((r) => r.metrics?.[name])?.metrics[name];
      const sorted = [...vals].sort((a, b) => a - b);
      const mid = median(vals);
      metrics[name] = {
        value: mid,
        unit: proto.unit,
        higher_is_better: proto.higher_is_better,
        n: vals.length,
        // The measured extremes, not merely the distance between them.
        //
        // `spread` alone cannot place an interval on a chart. The median of
        // three repetitions is the middle MEASUREMENT, not the midpoint of the
        // range, so a site drawing `value ± spread / 2` would draw an interval
        // whose ends the harness never observed — and would do it worst exactly
        // where the repetitions are most skewed, which is where a reader most
        // needs the truth. The environment's own caveat records run-to-run
        // spread reaching 14.5%, which is wider than most of the differences
        // this page exists to show, so the interval is not decoration.
        //
        // At three repetitions `lo`, `value` and `hi` ARE the three
        // measurements; `values` carries them explicitly so the site keeps
        // drawing every repetition if `reps` ever rises. Cost is bounded by
        // arms x metrics x reps — the same order as the metrics map it sits in.
        lo: sorted[0],
        hi: sorted[sorted.length - 1],
        values: sorted,
        // Relative range, or `null` when it has no meaning.
        //
        // This was `(max - min) / median` unguarded, and every all-zero metric
        // — `duplicate_rows` on a clean run, which is every published run —
        // computed 0/0 = NaN. NaN is not representable in JSON, so Docusaurus's
        // global data serialised it to `null`, and the component's
        // `(spread * 50).toFixed(1)` then rendered it as "±0.0%": a precision
        // claim about a quantity that was never measurable. Undefined is said
        // rather than implied.
        spread:
          vals.length < 2 ? 0 : mid === 0 ? null : (sorted[sorted.length - 1] - sorted[0]) / mid,
      };
    }
    rows.push({
      key,
      group: groupKey(newest),
      entrant: newest.sut.entrant,
      variant_id: newest.sut.variant_id,
      version: newest.sut.version ?? null,
      commit: newest.sut.commit ?? null,
      image_digest: newest.sut.image_digest,
      env_id: newest.run.env_id,
      harness_version: newest.run.harness_version,
      dataset_version: newest.run.dataset_version,
      ts_ms: newest.run.ts_ms,
      // Carried so the page can honour the contract without re-deriving any of
      // it: `approach` decides headline eligibility, `status` decides whether an
      // arm may be ranked at all, and `wire_format` is required beside every
      // number by rule 5.
      status: worstStatus(counted),
      mode: newest.variant?.mode ?? null,
      // Fail closed. A record that does not say what it is cannot be
      // headline-eligible: defaulting to `realistic` meant a foreign or
      // hand-edited record was ranked by default, which is the wrong direction
      // for the valve rule 3 exists to be.
      approach: newest.variant?.approach ?? 'undeclared',
      wire_format: newest.variant?.wire_format ?? null,
      reps_counted: counted.length,
      // The union across repetitions, not the newest one's. A caveat that
      // applied to any repetition applies to the number they were medianed into
      // — a throttled rep does not stop being throttled because the next one
      // was not.
      flags: [...new Set(counted.flatMap((r) => r.flags || []))].sort(),
      metrics,
    });
  }
  rows.sort((a, b) => b.ts_ms - a.ts_ms);
  attempts.sort((a, b) => b.ts_ms - a.ts_ms);
  return { rows, attempts };
}

module.exports = function benchData(context) {
  const root = repoRoot(context.siteDir);

  return {
    name: PLUGIN,

    getPathsToWatch() {
      return [
        path.join(root, 'entrants/**/entrant.toml'),
        path.join(root, 'environments/*.toml'),
        path.join(root, 'results/**/*.jsonl'),
      ];
    },

    async loadContent() {
      const entrants = loadEntrants(root);
      const environments = loadEnvironments(root);
      const { records, counts } = loadRecords(root);
      const { rows, attempts } = summarise(records);

      const groups = [...new Set(rows.map((r) => r.group))]
        .map((g) => {
          const any = rows.find((r) => r.group === g);
          return {
            key: g,
            env_id: any.env_id,
            harness_version: any.harness_version,
            dataset_version: any.dataset_version,
          };
        })
        // Ordered by key so an unchanged tree builds to an identical list — the
        // page re-sorts by richness before rendering, so this order only has to
        // be deterministic, not meaningful.
        .sort((a, b) => a.key.localeCompare(b.key));

      // Build determinism: derived from the newest record rather than the wall
      // clock, so rebuilding an unchanged tree produces an identical site and a
      // diff means something changed.
      const generatedAt = records.length
        ? new Date(Math.max(...records.map((r) => r.run?.ts_ms ?? 0))).toISOString()
        : null;

      return { entrants, environments, rows, attempts, groups, counts, generatedAt, root };
    },

    async contentLoaded({ content, actions }) {
      actions.setGlobalData({
        entrants: content.entrants,
        environments: content.environments,
        rows: content.rows,
        attempts: content.attempts,
        groups: content.groups,
        counts: content.counts,
        generatedAt: content.generatedAt,
      });

      // One profile page per declared system, generated from the descriptors.
      //
      // CONTRIBUTING.md promises that "adding entrant N+1 touches exactly one
      // new directory. There is no central registry to update." A hand-written
      // page per system would quietly break that: the twenty-first vendor would
      // owe the site a page as well as a descriptor, and pages written at
      // different times drift into flattering some systems more carefully than
      // others. So the route is derived, and every system gets the same shape.
      //
      // The module carries only the id. Everything else is already in global
      // data on every page, and shipping a second copy of a system's rows here
      // would put the same numbers in the bundle twice — which is exactly the
      // payload problem the header comment on `summarise` is about.
      //
      // Note what does NOT happen here: no entrant id appears as a literal.
      // They come from the descriptors that were just parsed, which is what
      // keeps `plugins/neutrality.test.js` green and the neutrality claim
      // checkable rather than asserted.
      for (const e of content.entrants) {
        const id = e.entrant.id;
        const profile = await actions.createData(
          `system-${id}.json`,
          JSON.stringify({ id }),
        );
        actions.addRoute({
          path: `${context.baseUrl}systems/${id}`,
          component: '@site/src/components/Results/system.tsx',
          modules: { profile },
          exact: true,
        });
      }
    },
  };
};

// Test-only. The plugin's contract is the Docusaurus hook above; these are the
// pure decisions inside it, exposed so `index.test.js` can pin behaviour that is
// not reachable through `loadContent` today — a status this build does not know
// is filtered out upstream by `CARRIES_METRICS`, so the fail-closed severity
// rule can only be exercised directly. Not part of the public surface.
module.exports.__testonly = {worstStatus, severity, groupKey, variantKey, UNKNOWN_SEVERITY};
