# Security

## Reporting a vulnerability

Use [**private vulnerability
reporting**](https://github.com/MarcusKainth/spate-benchmark/security/advisories/new).
It opens a private advisory visible only to the maintainers, and it is the
channel for anything you would not want in a public issue.

Expect an acknowledgement within seven days. If a report is valid you will be
credited in the advisory unless you ask not to be.

Please do not open a public issue for a security report. Everything else —
including "this arm is configured badly", which is the contribution this project
most wants — belongs in a normal issue or pull request. See
[CONTRIBUTING.md](CONTRIBUTING.md).

## What this project is, and what that makes interesting

This is a benchmark harness and a static site. It is not a service, it holds no
user data, and it has no users to attack. The asset worth protecting is the
**integrity of a published number**.

So the reports that matter here are the ones that would let a number be believed
when it should not be. In scope:

- A way to make `bench run` record a measurement that did not happen, or record
  one under a different arm, variant or environment than the one that produced
  it.
- A way to defeat the comparability keys — `harness_version`, `dataset_version`,
  `env_id`, `env_digest` — so that records taken under different protocols,
  corpora or hardware are drawn on one axis.
- A way to get a fabricated or tampered record past `bench validate`, or to
  break the `run_id` uniqueness that keeps a union-merged results file from
  silently double-counting a measurement into a published median.
- A way for an entrant descriptor, a workload file or a result shard to execute
  code in the harness, in CI, or at site build time.
- Anything that lets a pull request from a fork reach a repository secret or the
  production deployment.

If you find one of these, say so even if you cannot show a full exploit. A
credible argument that a number could be wrong is worth more to this project than
a proof-of-concept against something that does not affect a result.

## Out of scope

**The harness runs container images and shells out to `docker` against your own
daemon socket.** That is the design. It builds images from Dockerfiles in this
repository, runs entrant containers, mounts the cgroup filesystem to sample them,
and executes a Python program inside a stock image to measure the broker. Anyone
who can run `bench` can already run code on that machine, and anyone who can
change a file in `entrants/` can change what those containers do.

So "the harness can run code on the machine running the harness" is not a
vulnerability, and neither is "a malicious entrant directory could do something
bad" — reviewing what an arm does before merging it is what code review is for.
This is a local benchmarking tool, not a sandbox, and it should not be pointed at
a Docker daemon you do not control.

Also out of scope: results are public data and are meant to be copied; the site
is static, has no accounts, no forms and no analytics; and the harness talks
plain HTTP to services on a private Docker network, which is deliberate and
documented in `harness/src/http.rs`.

## Dependency advisories

`npm audit` reports findings against `website/`. Every one of them is a build-time
or dev-time transitive of Docusaurus, and none reaches anything published: the site
is prerendered to static HTML, CSS and JavaScript and served by Cloudflare Pages,
and `docusaurus.config.ts` sets `future.faster`, so the build runs on Rspack, SWC
and Lightning CSS rather than webpack, Babel and Terser. Installs come from the
committed lockfile with `ignore-scripts` set in `website/.npmrc`.

Where an advisory has a reachable fix, `website/package.json` carries an
`overrides` entry pinning past it rather than waiting for Docusaurus to bump the
parent that holds it back. The overrides are global rather than scoped to a
parent, deliberately: the claim being made is that this tree contains no
vulnerable copy at all, and a scoped override would let some future parent
reintroduce one quietly.

- **`serialize-javascript`**, pinned to `^7.0.7` (RCE via `RegExp.flags`, plus a
  CPU-exhaustion DoS). Reached only from `css-minimizer-webpack-plugin` and
  `copy-webpack-plugin`, which `future.faster` takes off the build path anyway.
  Upstream fixed both those parents — they now want `^7.0.3` — but Docusaurus
  3.10.2 still pins the majors that want `^6`.
- **`uuid`**, pinned to `^11.1.1` (missing buffer bounds check in v3/v5/v6). Reached
  only from `sockjs` under `webpack-dev-server`, so only under a local
  `npm start`; and `sockjs` calls `v4()` with no `buf` argument, which is the sole
  affected path, so this one was never live here. `sockjs@0.3.24` is the latest
  release and still pins `^8.3.2`; the real fix is `webpack-dev-server@6`, which
  drops `sockjs` entirely, and Docusaurus pins 5.x. The `^11` line is also what
  `uuid`'s own deprecation notice directs CommonJS consumers to.

One finding has no fix available and is dismissed rather than pinned:

- **`brace-expansion`** (DoS via unbounded expansion). Reached from
  `@docusaurus/core` → `serve-handler` → `minimatch@3`, so only under a local
  `docusaurus serve`, over this repository's own files. The patch exists solely in
  `5.0.8`; `1.1.16` is the newest release on the 1.x line and there is no backport,
  and `minimatch@3` requires `^1.1.7` so it cannot reach 5.x. Forcing it is not
  available either — 1.x exports the function directly, 5.x exports a named
  `expand`, so `minimatch@3` would fail at its first call. It clears when either
  `brace-expansion` backports to 1.x or Docusaurus moves `serve-handler` off
  `minimatch@3`, and the alert is dismissed rather than added to
  `.github/dependabot.yml`'s ignore list so that a backport still raises a fresh
  one.

A dependency advisory that *does* reach the built site, or one against the Rust
harness, is worth reporting through the channel above.

## Supported versions

There are no releases and no version branches. `main` is the only supported
state, and a fix lands there.
