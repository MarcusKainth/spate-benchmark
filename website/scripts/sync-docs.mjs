// Renders the repository's normative documents into the docs tree.
//
// METHODOLOGY.md lives at the repository root because that is where an
// implementer of an arm reads it, and it is referenced from entrant descriptors,
// arm READMEs and Java sources. Copying its text into a second file for the site
// would create two sources that drift, and the one that drifts is always the one
// nobody is reading at the time.
//
// So it is copied at build time with front matter prepended, and the copy is
// gitignored. One source of truth; the site renders it.

import {mkdirSync, readFileSync, writeFileSync} from 'node:fs';
import {dirname, join} from 'node:path';
import {fileURLToPath} from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, '..', '..');
const docs = join(root, 'docs');

/** Root document → docs page, with the front matter the site needs. */
const SYNCED = [
  {
    from: 'METHODOLOGY.md',
    to: 'methodology.md',
    frontMatter: {
      id: 'methodology',
      title: 'Methodology',
      description:
        'The normative fairness contract every arm in this benchmark conforms to.',
      sidebar_label: 'Methodology',
    },
    // The heading is supplied by front matter, so the source's own H1 would
    // render twice.
    stripFirstHeading: true,
  },
];

function toFrontMatter(fields) {
  const lines = Object.entries(fields).map(([k, v]) =>
    typeof v === 'string' && (v.includes(':') || v.includes('\n'))
      ? `${k}: ${JSON.stringify(v)}`
      : `${k}: ${v}`,
  );
  return `---\n${lines.join('\n')}\n---\n\n`;
}

mkdirSync(docs, {recursive: true});

for (const spec of SYNCED) {
  const src = readFileSync(join(root, spec.from), 'utf8');
  let body = src;
  if (spec.stripFirstHeading) {
    body = body.replace(/^#\s+.*\n+/, '');
  }
  // Links in the root document are relative to the repository; on the site they
  // have to point at the repository on GitHub, since the site has no page for
  // `workload/schema/sensor_batch.avsc`.
  body = body.replace(
    /\]\((workload|entrants|environments|harness)\//g,
    '](https://github.com/MarcusKainth/spate-benchmark/blob/main/$1/',
  );

  const banner =
    `{/* Generated from ${spec.from} by website/scripts/sync-docs.mjs. ` +
    `Edit that file, not this one. */}\n\n`;

  writeFileSync(join(docs, spec.to), toFrontMatter(spec.frontMatter) + banner + body);
  process.stdout.write(`synced ${spec.from} -> docs/${spec.to}\n`);
}
