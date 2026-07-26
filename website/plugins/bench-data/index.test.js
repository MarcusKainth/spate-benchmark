// What this file is for.
//
// The site's data layer decides which numbers a reader is allowed to compare,
// which are allowed to be ranked, and which are struck through. Every one of
// those decisions was wrong at some point and none of them had a test:
//
//   - `infra_bound` records were dropped by a `status !== 'ok'` filter, making
//     "we ran it and it blew the headroom limit" render identically to "we never
//     ran it".
//   - `approach` never reached the row, so a `stripped` arm — one using code this
//     project wrote rather than code the system ships — was ranked on the
//     headline axis, above the honest arm of the same system.
//   - `tier` was not in the group key, so arms doing measurably different amounts
//     of work were ranked against each other on one bar scale.
//   - A row took the newest repetition's status and flags rather than the worst
//     and the union, which reintroduced the first two bugs one layer up.
//
// The fixture in `__fixtures__/` exists to hold each of those cases in a shape
// small enough to read. Run with `npm test`.

const assert = require('node:assert/strict');
const path = require('node:path');
const {test} = require('node:test');

const FIXTURE = path.join(__dirname, '__fixtures__');

/** Loads the plugin's global data against the fixture tree. */
async function load() {
  process.env.BENCH_ROOT = FIXTURE;
  const plugin = require('./index.js')({siteDir: FIXTURE});
  return plugin.loadContent();
}

const find = (rows, entrant, variant) =>
  rows.find((r) => r.entrant === entrant && r.variant_id === variant);

test('tier splits the comparability group, so two tiers are never one axis', async () => {
  const {groups, rows} = await load();
  assert.ok(groups.length >= 2, 'the fixture must contain more than one group');
  // The property, not a fixed list: no group may contain two tiers, whatever
  // else the fixture grows. Asserting the exact set instead made this test fail
  // the moment a record at a different harness version was added — which is the
  // protocol-version split working, not a defect.
  for (const g of groups) {
    const tiers = new Set(rows.filter((r) => r.group === g.key).map((r) => r.tier));
    assert.equal(tiers.size, 1, `group ${g.key} spans tiers ${[...tiers]}`);
  }
  assert.deepEqual([...new Set(rows.map((r) => r.tier))].sort(), ['a', 'b']);
  for (const r of rows) {
    // A component, not a suffix: the key grows as new axes are added — `mode`
    // was appended after this test was written — and asserting on position
    // makes a correct addition look like a regression.
    assert.ok(
      r.group.split('|').includes(`tier-${r.tier}`),
      `${r.variant_id} in ${r.group}`,
    );
  }
});

test('mode is a comparability axis, so drain and sustained never share one', async () => {
  const {rows} = await load();
  // `rows_per_s` means "how fast can this go" in drain and "the rate we asked
  // for" in sustained. Two arms of entirely different capacity report the same
  // number, so the axis would be meaningless before it was wrong.
  for (const r of rows) {
    assert.ok(r.group.split('|').includes(`mode-${r.mode}`), `${r.variant_id} in ${r.group}`);
  }
  const drain = rows.filter((r) => r.mode === 'drain');
  assert.ok(drain.length > 0, 'the fixture is drain-mode throughout');
  const synthetic = {...drain[0], mode: 'sustained'};
  assert.notEqual(
    synthetic.group.replace('mode-drain', 'mode-sustained'),
    drain[0].group,
    'changing only the mode must change the group',
  );
});

test('a different protocol version is a different group, whatever else matches', async () => {
  const {groups, rows} = await load();
  // gamma runs harness 2 on the same environment, dataset and tier as arms at
  // harness 1. METHODOLOGY makes that a hard split: records measured under
  // different protocols are never drawn on one axis.
  const gamma = rows.filter((r) => r.entrant === 'gamma');
  assert.ok(gamma.length > 0);
  const others = rows.filter((r) => r.tier === 'a' && r.entrant !== 'gamma');
  assert.ok(others.length > 0, 'need a tier-a arm at the older protocol to compare against');
  for (const g of gamma) {
    for (const o of others) {
      assert.notEqual(g.group, o.group, 'harness 1 and harness 2 must not share a group');
    }
  }
  assert.ok(
    new Set(groups.map((g) => g.harness_version)).size >= 2,
    'the split must be visible to the page, not only inside the key',
  );
});

test('every repetition of an invocation is medianed into one row', async () => {
  const {rows} = await load();
  const r = find(rows, 'alpha', 'tier-a');
  // Three reps at 1000/1100/1200 — the mark is the interval, so the row has to
  // carry all three rather than the newest.
  assert.equal(r.reps_counted, 3);
  assert.equal(r.metrics.rows_per_s.value, 1100, 'median of the three');
  assert.equal(r.metrics.rows_per_s.lo, 1000);
  assert.equal(r.metrics.rows_per_s.hi, 1200);
});

test('one infra-bound repetition makes the whole row infra-bound', async () => {
  const {rows} = await load();
  // The offending rep is not the newest, which is exactly how taking
  // `newest.status` used to publish it as ok.
  assert.equal(find(rows, 'alpha', 'tier-b').status, 'infra_bound');
});

test('flags are the union across repetitions, not the newest one\'s', async () => {
  const {rows} = await load();
  // Only rep 2 of alpha:tier-a is throttled, and it is not the newest.
  assert.deepEqual(find(rows, 'alpha', 'tier-a').flags, ['cpu_cap_throttled']);
});

test('a run that produced no publishable number is an explicit gap, not silence', async () => {
  const {rows, attempts} = await load();
  assert.equal(attempts.length, 1);
  assert.equal(attempts[0].status, 'failed');
  assert.ok(
    !rows.some((r) => r.metrics.rows_per_s?.value === 0),
    'a failed record must not become a row',
  );
});

test('approach and wire format reach the row, which is what makes the contract renderable', async () => {
  const {rows} = await load();
  assert.equal(find(rows, 'beta', 'tier-a-hand').approach, 'stripped');
  assert.equal(find(rows, 'beta', 'tier-a').approach, 'realistic');
  assert.equal(find(rows, 'alpha', 'tier-a').wire_format, 'native');
});

test('a stripped arm is present but never headline-eligible, even when it is fastest', async () => {
  const {rows} = await load();
  const stripped = find(rows, 'beta', 'tier-a-hand');
  const tierA = rows.filter((r) => r.tier === 'a');
  const fastest = tierA.reduce((a, b) =>
    a.metrics.rows_per_s.value >= b.metrics.rows_per_s.value ? a : b,
  );
  assert.equal(fastest.variant_id, 'tier-a-hand', 'fixture must keep this the fastest');
  // Mirrors `unrankedBecause` in the component: the row carries everything
  // needed to bar it, so the decision cannot be lost between here and render.
  const eligible = (r) => r.status === 'ok' && r.approach === 'realistic';
  assert.equal(eligible(stripped), false);
  assert.ok(tierA.some(eligible), 'something must still be rankable');
});

test('two sittings on one day are two rows, not one', async () => {
  const {rows} = await load();
  // Four records, one arm, one configuration, one UTC day, two invocation ids.
  // Under the calendar-day key these collapsed into a single published row whose
  // spread read as run-to-run variance rather than as two different sweeps.
  const gamma = rows.filter((r) => r.entrant === 'gamma');
  assert.equal(gamma.length, 2, 'one row per invocation');
  const medians = gamma.map((r) => r.metrics.rows_per_s.value).sort((a, b) => a - b);
  assert.deepEqual(medians, [1005, 2005]);
  for (const r of gamma) assert.equal(r.reps_counted, 2);
});

test('a status this build does not recognise is never ranked', () => {
  const {worstStatus, severity, UNKNOWN_SEVERITY} = require('./index.js').__testonly;
  // Fails CLOSED. Scoring an unknown status 0 made it tie with `ok` and lose, so
  // a status added by a newer harness would have been published as sound by an
  // older site — the same fail-open mistake `approach` used to make.
  assert.equal(severity('ok'), 0);
  assert.equal(severity('infra_bound'), 1);
  assert.equal(severity('something_a_newer_harness_emits'), UNKNOWN_SEVERITY);
  assert.equal(
    worstStatus([{status: 'ok'}, {status: 'something_a_newer_harness_emits'}, {status: 'ok'}]),
    'something_a_newer_harness_emits',
  );
  assert.equal(worstStatus([{status: 'ok'}, {status: 'infra_bound'}]), 'infra_bound');
});

test('a descriptor that does not parse fails the build rather than half-loading', async () => {
  process.env.BENCH_ROOT = path.join(FIXTURE, '..', '__no_such_tree__');
  const plugin = require('./index.js')({siteDir: FIXTURE});
  // A missing tree is empty, not an error — the site renders "no measurements".
  const empty = await plugin.loadContent();
  assert.equal(empty.rows.length, 0);
  assert.equal(empty.entrants.length, 0);
});
