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

const PLUGIN = 'bench-data';

/** Repository root, relative to `website/`. */
function repoRoot(siteDir) {
  return process.env.BENCH_ROOT || path.resolve(siteDir, '..');
}

/**
 * A deliberately small TOML reader.
 *
 * The descriptors are read by the Rust harness with a real parser, and that is
 * the authority — `entrants_are_valid` fails the build if a descriptor is
 * malformed, so by the time the site sees one it has already been validated.
 * This only needs the subset the site renders, and pulling a TOML dependency
 * into the site's tree to re-do work that is already gated is not worth the
 * supply-chain surface.
 */
function parseToml(src) {
  const root = {};
  let cursor = root;
  let arrayMode = false;

  const setPath = (obj, keys, value) => {
    let o = obj;
    for (const k of keys.slice(0, -1)) {
      o[k] = o[k] || {};
      o = o[k];
    }
    o[keys[keys.length - 1]] = value;
  };

  const scalar = (raw) => {
    const v = raw.trim();
    if (v === 'true') return true;
    if (v === 'false') return false;
    if (/^-?\d+$/.test(v)) return Number(v);
    if (/^-?\d*\.\d+$/.test(v)) return Number(v);
    if (v.startsWith('[')) {
      const inner = v.slice(1, v.lastIndexOf(']'));
      if (!inner.trim()) return [];
      return inner.split(',').map((x) => scalar(x)).filter((x) => x !== '');
    }
    if (v.startsWith('{')) {
      const out = {};
      const inner = v.slice(1, v.lastIndexOf('}'));
      for (const pair of splitTopLevel(inner)) {
        const eq = pair.indexOf('=');
        if (eq > 0) out[pair.slice(0, eq).trim()] = scalar(pair.slice(eq + 1));
      }
      return out;
    }
    return v.replace(/^["']|["']$/g, '');
  };

  const lines = src.split('\n');
  for (let i = 0; i < lines.length; i += 1) {
    let line = lines[i];
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith('#')) continue;

    const arrayHeader = trimmed.match(/^\[\[(.+)\]\]$/);
    if (arrayHeader) {
      const keys = arrayHeader[1].split('.');
      let o = root;
      for (const k of keys.slice(0, -1)) {
        o[k] = o[k] || {};
        o = o[k];
      }
      const last = keys[keys.length - 1];
      o[last] = o[last] || [];
      cursor = {};
      o[last].push(cursor);
      arrayMode = true;
      continue;
    }
    const header = trimmed.match(/^\[(.+)\]$/);
    if (header) {
      const keys = header[1].split('.');
      let o = root;
      for (const k of keys) {
        o[k] = o[k] || {};
        o = o[k];
      }
      cursor = o;
      arrayMode = false;
      continue;
    }

    const eq = line.indexOf('=');
    if (eq < 0) continue;
    const key = line.slice(0, eq).trim();
    let value = line.slice(eq + 1);

    // Multi-line basic strings. Descriptors use them heavily for the `notes` and
    // `why` fields, which are the most valuable content in the file.
    if (value.trim().startsWith('"""')) {
      const parts = [value.trim().slice(3)];
      while (i + 1 < lines.length && !parts.join('\n').includes('"""')) {
        i += 1;
        parts.push(lines[i]);
      }
      const joined = parts.join('\n');
      setPath(arrayMode ? cursor : cursor, [key], joined.slice(0, joined.indexOf('"""')).trim());
      continue;
    }
    setPath(cursor, [key], scalar(value));
  }
  return root;
}

/** Splits on commas that are not inside brackets or quotes. */
function splitTopLevel(s) {
  const out = [];
  let depth = 0;
  let quote = null;
  let start = 0;
  for (let i = 0; i < s.length; i += 1) {
    const c = s[i];
    if (quote) {
      if (c === quote) quote = null;
    } else if (c === '"' || c === "'") quote = c;
    else if (c === '[' || c === '{') depth += 1;
    else if (c === ']' || c === '}') depth -= 1;
    else if (c === ',' && depth === 0) {
      out.push(s.slice(start, i));
      start = i + 1;
    }
  }
  out.push(s.slice(start));
  return out.map((x) => x.trim()).filter(Boolean);
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
    .map((p) => parseToml(fs.readFileSync(p, 'utf8')))
    .filter((s) => s.entrant && s.entrant.id)
    .sort((a, b) => (a.display?.order ?? 0) - (b.display?.order ?? 0));
}

function loadEnvironments(root) {
  const dir = path.join(root, 'environments');
  return readDirSafe(dir)
    .filter((e) => e.isFile() && e.name.endsWith('.toml'))
    .map((e) => parseToml(fs.readFileSync(path.join(dir, e.name), 'utf8')))
    .filter((s) => s.id);
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

/** The key that decides what may share an axis. */
function groupKey(rec) {
  return [
    rec.run?.env_id,
    rec.run?.harness_version,
    rec.run?.dataset_version,
    rec.run?.infra?.digest,
  ].join('|');
}

function median(xs) {
  if (!xs.length) return null;
  const s = [...xs].sort((a, b) => a - b);
  const m = Math.floor(s.length / 2);
  return s.length % 2 ? s[m] : (s[m - 1] + s[m]) / 2;
}

/**
 * One summary row per (group, entrant, variant, version).
 *
 * Repetitions within a single invocation are aggregated by median. Runs from
 * DIFFERENT sittings are not: they get their own rows, keyed by `run_id`'s
 * sitting via the record's own group. That is the correction to the framework
 * site's aggregator, which hashes only variant keys and so silently medians a
 * re-run months later into the original figure while captioning it with the
 * newest date.
 */
function summarise(records) {
  const byKey = new Map();
  for (const rec of records) {
    if (rec.status !== 'ok') continue;
    const key = [
      groupKey(rec),
      rec.sut?.entrant,
      rec.sut?.variant_id,
      rec.sut?.version ?? rec.sut?.commit ?? '?',
      // Distinct sittings stay distinct. Without this, a re-run silently joins
      // the original and the archive stops being a history.
      new Date(rec.run?.ts_ms ?? 0).toISOString().slice(0, 10),
    ].join('|');
    if (!byKey.has(key)) byKey.set(key, []);
    byKey.get(key).push(rec);
  }

  const rows = [];
  for (const [key, reps] of byKey) {
    const newest = reps.reduce((a, b) => (a.run.ts_ms >= b.run.ts_ms ? a : b));
    const metrics = {};
    const names = new Set(reps.flatMap((r) => Object.keys(r.metrics || {})));
    for (const name of names) {
      const vals = reps.map((r) => r.metrics?.[name]?.value).filter((v) => typeof v === 'number');
      if (!vals.length) continue;
      const proto = reps.find((r) => r.metrics?.[name])?.metrics[name];
      metrics[name] = {
        value: median(vals),
        unit: proto.unit,
        higher_is_better: proto.higher_is_better,
        n: vals.length,
        spread: vals.length > 1 ? (Math.max(...vals) - Math.min(...vals)) / median(vals) : 0,
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
      flags: newest.flags || [],
      superseded_by: newest.superseded_by || null,
      metrics,
    });
  }
  rows.sort((a, b) => b.ts_ms - a.ts_ms);
  return rows;
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
      const rows = summarise(records);

      const groups = [...new Set(rows.map((r) => r.group))].map((g) => {
        const any = rows.find((r) => r.group === g);
        return {
          key: g,
          env_id: any.env_id,
          harness_version: any.harness_version,
          dataset_version: any.dataset_version,
        };
      });

      // Build determinism: derived from the newest record rather than the wall
      // clock, so rebuilding an unchanged tree produces an identical site and a
      // diff means something changed.
      const generatedAt = records.length
        ? new Date(Math.max(...records.map((r) => r.run?.ts_ms ?? 0))).toISOString()
        : null;

      return { entrants, environments, rows, groups, counts, generatedAt, root };
    },

    async contentLoaded({ content, actions }) {
      actions.setGlobalData({
        entrants: content.entrants,
        environments: content.environments,
        rows: content.rows,
        groups: content.groups,
        counts: content.counts,
        generatedAt: content.generatedAt,
      });
    },
  };
};
