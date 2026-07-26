//! The ceiling pass's consumer, run from **inside** the bench network.
//!
//! # The confounder this module closes
//!
//! The broker consume ceiling used to be read by `rdkafka` in the harness
//! process on macOS, through Docker Desktop's published port 9092. Every arm
//! consumes container-to-container over [`NETWORK`] and never crosses that
//! boundary, so the two are not the same fetch and the ceiling was measuring the
//! rig.
//!
//! It is the identical defect [`crate::inserter`] closed on the ingest side, in
//! the other direction, and the same host is responsible for both: measured
//! here, the published-port path carried 246–279 MB/s whatever was sent through
//! it, while the container-to-container path carried 2.6–3.1 GB/s. A consume
//! ceiling taken on the slow side of that boundary is a **floor** — and it was
//! the nearest ceiling to the fastest arms, which is the one place a floor
//! cannot be tolerated.
//!
//! What it cost, measured: the same corpus, the same broker, the same partition
//! count and the same window read 72,349 messages per second from the host and
//! 1,719,373 from a container on [`NETWORK`]. Twenty-four times, and every share
//! computed against the smaller number was twenty-four times too large.
//!
//! # Why a Python program fed to a stock image, again
//!
//! The shape [`crate::inserter`] and [`crate::sampler`] both use: a measurement
//! should not need a purpose-built image, because an image is a thing to build,
//! to version and to get wrong. `python:3.12-alpine` is already pulled for the
//! inserter and nothing about it is under measurement.
//!
//! The alternatives were tried on this host rather than reasoned about, and each
//! was rejected by a number:
//!
//! * **`rpk` inside the running Redpanda container.** It would share the
//!   broker's own CPU cap with the broker, so the client and the target would be
//!   competing for the thing being measured.
//! * **`rpk` in a container of its own**, from the image the infrastructure
//!   already runs. This works and is fast — 1.03 GB/s in one process, 4.24 GB/s
//!   across eight — but it has no aggregate: it prints one line per record, so
//!   the counting has to happen somewhere. Counting on the host puts the totals
//!   back across the boundary this module exists to leave, and it costs what
//!   that boundary costs: piping four bytes per record to the harness dropped
//!   the same eight-way read from 4.24 GB/s to 2.66 GB/s. Counting inside the
//!   container needs an interpreter, and the Redpanda image has no Python.
//! * **A `librdkafka` client installed at pass time**, or a Rust probe built into
//!   an image of its own. Both are a build or a package fetch inside a
//!   measurement, and the first is a network dependency at the moment the rig is
//!   supposed to be quiet.
//!
//! So this program speaks Kafka's fetch protocol directly, on stdlib sockets,
//! and counts inside the container — which is also why the pipe carries one line
//! per window rather than one per record.
//!
//! # Why it is not the constraint, measured
//!
//! The ingest pass proved its inserter against `ENGINE = Null`, a target that
//! cannot be the bottleneck, before trusting the ceiling it produced. The
//! equivalent here is the broker's own cgroup: if the broker is not at its cap
//! while this client is reading flat out, then the broker is not the constraint
//! and the figure is this client's own capability rather than the broker's.
//!
//! On this host, eight fetchers over eight partitions read **1,719,373 messages
//! per second, 6.99 GB/s**, while `spate-bench-redpanda` sat at 4.29 of the 8
//! cores it held **at the time of that reading** — 54% of its cap, never
//! throttled. The broker has since been given 4 cores instead of 8; the figure
//! is kept as it was taken, because a measurement rewritten to match a later
//! allocation is no longer a measurement. So 6.99 GB/s is a *lower bound on
//! this client*, taken against a broker demonstrably doing less than it could.
//! The fastest arm in `results/` consumes 40,562 messages per second, 165 MB/s.
//! The client is therefore **42x the fastest arm's consume path**, and the
//! figure it produces understates the broker rather than the reverse — the same
//! direction of error [`crate::ceiling`] points every other number in.
//!
//! For scale against the alternatives above: this client is 1.6x `rpk`'s
//! eight-way in-container read and 2.6x `rpk`'s read once the counting crosses
//! the pipe.
//!
//! # What the protocol costs, and what it buys
//!
//! Speaking the wire protocol rather than using a client library means the
//! program can be wrong in ways a library could not be, so it is not trusted:
//! every record batch carries the broker's own record count and every record is
//! walked, and a batch whose walked count disagrees with its declared one fails
//! the window rather than reporting a number. The mean framed message size the
//! walk produces is checked by [`crate::ceiling`] against what the generator
//! emits, which is the same check that keeps a foreign topic from being measured
//! by mistake — and which a mis-parse would fail.
//!
//! What it buys, beyond speed, is that end-of-backlog is the broker's own
//! statement rather than a client's flag: every fetch response carries the
//! partition's high watermark, so a partition that runs dry inside the window is
//! detected by comparing the next offset against it. That is the `DRAINED`
//! refusal, and it is why the window can be sized against the backlog at all.
//!
//! # No group, no commits, and the corpus survives
//!
//! The program issues raw `ListOffsets` and `Fetch` calls and never joins a
//! consumer group, so there is no rebalance inside the window and no offset is
//! ever committed. Every window starts again from each partition's earliest
//! offset. The corpus lives inside the broker and a rig that consumed it
//! destructively would cost every subsequent arm its input.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::Duration;

use crate::docker::{NETWORK, docker_try};
use crate::infra::Endpoints;

/// The fetcher container's name. Fixed, so an orphan from an interrupted pass is
/// removed by the next one rather than accumulating — and so that a half-finished
/// pass cannot leave eight processes reading the broker the next measurement is
/// about to time.
const FETCHER_CONTAINER: &str = "spate-bench-fetcher";

/// Image the fetcher runs in. Chosen only because it has a Python interpreter,
/// and because [`crate::inserter`] has already pulled it.
const FETCHER_IMAGE: &str = "python:3.12-alpine";

/// Seconds one broker call may take before the fetcher gives up on it.
///
/// The same bound the inserter uses, for the same reason: a broker that stops
/// answering must fail a window instead of hanging a pass that holds the arm
/// lock.
const FETCH_TIMEOUT_S: u64 = 120;

/// What one window read, as the fetcher counted it.
///
/// Counted inside the container, on the container's own monotonic clock, because
/// the window is a property of the fetching processes rather than of the process
/// that asked for them. A harness-side clock would additionally charge the window
/// for the pipe round trip that starts and ends it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RemoteFetch {
    /// Messages the broker served inside the window.
    pub msgs: u64,
    /// Bytes of message **payload** those messages carried.
    ///
    /// Payload only: keys, record framing and batch headers are excluded, so the
    /// byte rate is a slight understatement of what the broker moved and the gate
    /// built on it is slightly strict rather than slightly lenient. It is also
    /// the quantity `corpus::frame_confluent` produces, which is what lets
    /// [`crate::ceiling`] check the topic against the generator.
    pub bytes: u64,
    /// Seconds the window ran for, from the barrier to the last complete fetch.
    pub elapsed_s: f64,
}

/// A running fetcher container, driven one window at a time over a pipe.
///
/// Deliberately not clever, exactly as [`crate::inserter::Inserter`] is not: it
/// holds a connection to the broker and it reads whatever partition split it is
/// told to for whatever window it is told to. Which partitions go to which
/// consumer, how long the window may be, whether the message size is this
/// corpus's and whether a drained partition is a refusal all stay in
/// [`crate::ceiling`], where they are tested without a broker.
#[derive(Debug)]
pub struct Fetcher {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    stopped: bool,
    depths: Vec<u64>,
}

impl Fetcher {
    /// Starts a fetcher on [`NETWORK`] and reads back what the topic holds.
    ///
    /// The per-partition backlog comes back at start-up because the window has to
    /// be sized against it: at the rate this client reads, the whole corpus is
    /// under a second of backlog, and a window longer than that measures an idle
    /// broker for the remainder. See [`Fetcher::depths`].
    ///
    /// # Errors
    ///
    /// If the container will not start, if the broker cannot be reached from
    /// inside the network, or if the program answers with anything but its ready
    /// line.
    pub fn start(ep: &Endpoints, topic: &str, partitions: i32) -> Result<Self, String> {
        // An orphan from an interrupted pass would hold the name.
        let _ = docker_try(&["rm", "-f", FETCHER_CONTAINER]);

        let partitions_arg = partitions.to_string();
        let timeout = FETCH_TIMEOUT_S.to_string();
        let mut child = Command::new("docker")
            .args([
                "run",
                "--rm",
                "-i",
                "--name",
                FETCHER_CONTAINER,
                // The whole point of this module.
                "--network",
                NETWORK,
                FETCHER_IMAGE,
                "python3",
                "-c",
                FETCHER_SRC,
                // The internal bootstrap, never the published one: reaching the
                // broker by container name over the bench network is the arms'
                // own path and the only path this figure may describe.
                &ep.bootstrap_internal,
                topic,
                &partitions_arg,
                &timeout,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Inherited rather than piped, as the inserter's is: a Python
            // traceback is the first thing an operator needs and the last thing
            // that should be buffered inside a struct nobody reads.
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("spawn the fetcher container: {e}"))?;

        let stdin = child.stdin.take().ok_or("the fetcher has no stdin")?;
        let stdout = BufReader::new(child.stdout.take().ok_or("the fetcher has no stdout")?);
        // Assembled before the ready line is read, so that a failure while
        // waiting for it drops a value that removes the container rather than
        // leaking one holding a pipe.
        let mut fetcher = Self {
            child,
            stdin: Some(stdin),
            stdout,
            stopped: false,
            depths: Vec::new(),
        };
        fetcher.depths = parse_ready(&fetcher.reply()?)?;
        Ok(fetcher)
    }

    /// How many messages each partition holds, in partition order.
    ///
    /// Read from the broker's own `ListOffsets` rather than assumed from the
    /// prefill, and per partition rather than in total, because the window is
    /// sized against the **shallowest** partition: a consumer whose partitions
    /// run dry first ends the window for everybody.
    #[must_use]
    pub fn depths(&self) -> &[u64] {
        &self.depths
    }

    /// Runs one window: one consumer per entry in `split`, reading those
    /// partitions from their earliest offset for `window`.
    ///
    /// # Errors
    ///
    /// If the fetcher cannot be reached, if any consumer failed, or if any
    /// partition ran out of backlog before the window closed. Every one of those
    /// is a refusal rather than a smaller number: a window that spent part of
    /// itself against an exhausted partition measures an idle broker, which is
    /// the defect the original rig documented and refused with `DRAINED`.
    pub fn burst(&mut self, split: &[Vec<i32>], window: Duration) -> Result<RemoteFetch, String> {
        let command = format_run(split, window);
        let stdin = self.stdin.as_mut().ok_or("the fetcher has been stopped")?;
        stdin
            .write_all(command.as_bytes())
            // Flushed rather than left to the pipe's own buffering: the reply
            // below is read synchronously, so an unflushed command is a deadlock
            // rather than a delay.
            .and_then(|()| stdin.flush())
            .map_err(|e| format!("ask the fetcher for a window: {e}"))?;
        parse_fetch(&self.reply()?)
    }

    /// One line from the fetcher.
    fn reply(&mut self) -> Result<String, String> {
        let mut line = String::new();
        match self.stdout.read_line(&mut line) {
            Ok(0) => Err(format!(
                "the fetcher exited without answering. Its stderr is on this terminal. It \
                 reads from inside {NETWORK}, so a failure here is the container, the image \
                 or the broker rather than the corpus."
            )),
            Ok(_) => Ok(line.trim().to_owned()),
            Err(e) => Err(format!("read from the fetcher: {e}")),
        }
    }

    /// Stops the fetcher, exactly once.
    ///
    /// Removes the container by name rather than killing the `docker` client:
    /// killing the client detaches it and leaves the container alive, which
    /// [`crate::sampler`] documents having paid for.
    fn shutdown(&mut self) {
        if std::mem::replace(&mut self.stopped, true) {
            return;
        }
        drop(self.stdin.take());
        let _ = docker_try(&["rm", "-f", FETCHER_CONTAINER]);
        let _ = self.child.wait();
    }
}

impl Drop for Fetcher {
    /// Every path out of a consume pass ends the container, including the
    /// refusals. A pass that abandoned a window — a drained partition, a broker
    /// that stopped answering — would otherwise leave eight consumers reading the
    /// broker the ingest pass is about to be measured beside.
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// The `RUN` line for one window: the seconds, then one comma-separated
/// partition list per consumer.
///
/// The split is computed by [`crate::ceiling`] and sent verbatim rather than
/// derived in the container, so the rule that a consumer with no partitions
/// drags the aggregate down stays where a test can reach it.
fn format_run(split: &[Vec<i32>], window: Duration) -> String {
    let mut line = format!("RUN {:.3}", window.as_secs_f64());
    for consumer in split {
        line.push(' ');
        line.push_str(
            &consumer
                .iter()
                .map(i32::to_string)
                .collect::<Vec<_>>()
                .join(","),
        );
    }
    line.push('\n');
    line
}

/// Reads the `READY` line's per-partition depths.
fn parse_ready(line: &str) -> Result<Vec<u64>, String> {
    let mut fields = line.split_whitespace();
    if fields.next() != Some("READY") {
        return Err(format!(
            "the fetcher answered {line:?} instead of READY. It reads from inside {NETWORK}, \
             so a failure here is the container or the broker rather than the corpus."
        ));
    }
    let depths: Result<Vec<u64>, _> = fields.map(str::parse::<u64>).collect();
    let depths = depths.map_err(|e| format!("the fetcher's partition depths do not parse: {e}"))?;
    if depths.is_empty() {
        return Err("the fetcher reported no partitions at all".to_owned());
    }
    Ok(depths)
}

/// Reads one `OK msgs bytes elapsed` reply, or turns an `ERR` into a refusal.
fn parse_fetch(line: &str) -> Result<RemoteFetch, String> {
    let mut fields = line.split_whitespace();
    match fields.next() {
        Some("OK") => {
            let mut next = |what: &str| -> Result<String, String> {
                fields
                    .next()
                    .map(str::to_owned)
                    .ok_or_else(|| format!("the fetcher's reply carries no {what}: {line:?}"))
            };
            let msgs: u64 = next("message count")?
                .parse()
                .map_err(|e| format!("the fetcher's message count does not parse: {e}"))?;
            let bytes: u64 = next("byte count")?
                .parse()
                .map_err(|e| format!("the fetcher's byte count does not parse: {e}"))?;
            let elapsed_s: f64 = next("window")?
                .parse()
                .map_err(|e| format!("the fetcher's window does not parse: {e}"))?;
            if msgs == 0 || elapsed_s <= 0.0 {
                return Err(format!(
                    "REFUSED: the fetcher reported {msgs} messages over {elapsed_s}s, which is \
                     not a measurement of anything"
                ));
            }
            Ok(RemoteFetch {
                msgs,
                bytes,
                elapsed_s,
            })
        }
        // A drained partition is named as such rather than passed through as an
        // ordinary failure, because it is the one failure that is about the
        // CORPUS rather than about the broker or the rig — and the operator's
        // remedy is a shorter window or a deeper prefill rather than a re-run.
        Some("ERR") if line.contains("DRAINED") => Err(format!(
            "REFUSED (DRAINED): {}. The remainder of the window would have measured an idle \
             broker, so the figure is refused rather than reported low. The consume pass sizes \
             its window against the backlog for exactly this reason; a window this short still \
             draining means the corpus is shallower than the client is fast.",
            line.trim_start_matches("ERR").trim()
        )),
        // Everything else passes through verbatim. The text is the broker's own
        // error code in the cases that matter, and rewording it here would cost
        // the caller the one string that says which of them happened.
        Some("ERR") => Err(line.trim_start_matches("ERR").trim().to_owned()),
        _ => Err(format!("the fetcher answered {line:?}")),
    }
}

/// The fetcher program.
///
/// Inline rather than in `workload/`, because it is not part of the workload: it
/// is this module's measurement rig, and the argument in the module docs rests on
/// what it does per record. Keeping the claim and the code that has to satisfy it
/// in one file is what makes that checkable by reading.
///
/// The protocol, in full. All lines are ASCII and terminated by `\n`.
///
/// * Out: `READY <depth0> <depth1> ...` once the broker has answered
///   `ListOffsets` for every partition.
/// * In: `RUN <seconds> <p,p,...> <p,p,...> ...`, one partition list per
///   consumer; out: `OK <msgs> <payload_bytes> <elapsed_s>` or `ERR <text>`.
/// * In: `QUIT`, or EOF, and the program exits.
///
/// One process per consumer rather than one thread, and that is load-bearing:
/// the per-record walk is the client's only real cost, it is pure Python, and
/// threads would serialise it behind one interpreter lock. Forked processes
/// scaled the read from 3.15 GB/s on one to 6.99 GB/s on eight; threads would not
/// have.
const FETCHER_SRC: &str = r##"
import os, socket, struct, sys, time

bootstrap, topic, partitions, timeout = (
    sys.argv[1], sys.argv[2], int(sys.argv[3]), float(sys.argv[4]))
host, _, port = bootstrap.rpartition(":")
port = int(port)

# Milliseconds the broker may hold a fetch open waiting for min_bytes. Short,
# because inside the window a fetch that waits is a fetch that is not measuring
# anything, and the only reason it would wait is a partition that has run dry —
# which is the DRAINED refusal rather than something to be patient about.
FETCH_MAX_WAIT_MS = 100
FETCH_MIN_BYTES = 1
# Per partition and per response. Generous: a ceiling has to be the best the
# fetch path can do rather than the best it does at a default, and a bigger
# response is fewer round trips per byte. Measured on this host, 4 MiB gave
# 6.00 GB/s, 8 MiB 6.23 and 16 MiB 6.65.
PARTITION_MAX_BYTES = 16 << 20
FETCH_MAX_BYTES = 64 << 20

inp = sys.stdin.buffer


def command():
    line = inp.readline()
    if not line:
        raise SystemExit(0)
    return line.decode().split()


def reply(text):
    sys.stdout.write(text + "\n")
    sys.stdout.flush()


def put_str(out, s):
    b = s.encode()
    out.append(struct.pack(">h", len(b)))
    out.append(b)


class Broker:
    """One connection, speaking request header v1 and response header v0.

    ListOffsets v1 and Fetch v11 are deliberately below the flexible-version
    boundary (ListOffsets v6, Fetch v12), so no request or response carries
    tagged fields and the framing is fixed-width throughout."""

    def __init__(self):
        self.sock = socket.create_connection((host, port), timeout)
        self.sock.settimeout(timeout)
        self.sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        self.correlation = 0
        self.buf = bytearray(1 << 20)

    def send(self, api, version, body):
        self.correlation += 1
        head = [struct.pack(">hhi", api, version, self.correlation)]
        put_str(head, "spate-bench-ceiling")
        frame = b"".join(head + body)
        self.sock.sendall(struct.pack(">i", len(frame)) + frame)

    def recv(self):
        size = struct.unpack(">i", bytes(self.exactly(4)))[0]
        resp = self.exactly(size)
        got = struct.unpack_from(">i", resp, 0)[0]
        if got != self.correlation:
            raise RuntimeError("correlation %d is not %d" % (got, self.correlation))
        return resp[4:]

    def exactly(self, n):
        # The buffer is reused rather than reallocated per fetch, and the caller
        # gets a view into it rather than a copy: at these rates a copy of every
        # response is tens of gigabytes a second of memcpy for nothing. It is
        # safe because the pipelined loop below finishes reading a response
        # before it asks for the next one.
        if n > len(self.buf):
            self.buf = bytearray(n)
        view = memoryview(self.buf)[:n]
        at = 0
        while at < n:
            got = self.sock.recv_into(view[at:], n - at)
            if not got:
                raise RuntimeError("the broker closed the connection")
            at += got
        return view

    def offsets_at(self, want, timestamp):
        body = [struct.pack(">ii", -1, 1)]
        put_str(body, topic)
        body.append(struct.pack(">i", len(want)))
        for p in want:
            body.append(struct.pack(">iq", p, timestamp))
        self.send(2, 1, body)
        r = self.recv()
        at = 0
        (topics,) = struct.unpack_from(">i", r, at)
        at += 4
        out = {}
        for _ in range(topics):
            (name_len,) = struct.unpack_from(">h", r, at)
            at += 2 + name_len
            (n,) = struct.unpack_from(">i", r, at)
            at += 4
            for _ in range(n):
                p, err, _ts, off = struct.unpack_from(">ihqq", r, at)
                at += 22
                if err:
                    raise RuntimeError("list offsets partition %d: error %d" % (p, err))
                out[p] = off
        return out

    def fetch(self, offsets, order):
        # The partitions are named in a ROTATED order, one step per fetch. A
        # response is capped at FETCH_MAX_BYTES and the broker fills it in the
        # order it was asked, so a fixed order starves the partitions at the end
        # of the list: measured here, one consumer holding eight partitions
        # emptied partition 1 in 0.69s of a 1.22s window while others still had
        # backlog, which is a DRAINED refusal caused entirely by the shape of
        # the request. Rotating is what every real client does and what makes a
        # consumer holding several partitions read them evenly.
        body = [
            struct.pack(">iiiibii", -1, FETCH_MAX_WAIT_MS, FETCH_MIN_BYTES,
                        FETCH_MAX_BYTES, 0, 0, -1),
            struct.pack(">i", 1),
        ]
        put_str(body, topic)
        body.append(struct.pack(">i", len(order)))
        for p in order:
            body.append(struct.pack(">iiqqi", p, -1, offsets[p], -1, PARTITION_MAX_BYTES))
        body.append(struct.pack(">i", 0))
        put_str(body, "")
        self.send(1, 11, body)


def locate(r, offsets):
    """Walks the response and every record batch HEADER, and nothing else.

    Split from the per-record walk so the next fetch can go out before the
    expensive half runs: the batch header carries the base offset and the last
    offset delta, which is everything needed to ask for the next batch."""
    at = 10
    (topics,) = struct.unpack_from(">i", r, at)
    at += 4
    spans = []
    drained = None
    for _ in range(topics):
        (name_len,) = struct.unpack_from(">h", r, at)
        at += 2 + name_len
        (n,) = struct.unpack_from(">i", r, at)
        at += 4
        for _ in range(n):
            partition, err, high = struct.unpack_from(">ihq", r, at)
            # partition, error, high watermark, last stable offset, log start.
            at += 30
            (aborted,) = struct.unpack_from(">i", r, at)
            at += 4
            if aborted > 0:
                at += 16 * aborted
            # preferred read replica, then the record set.
            at += 4
            (size,) = struct.unpack_from(">i", r, at)
            at += 4
            if err:
                raise RuntimeError("fetch partition %d: error %d" % (partition, err))
            if size > 0:
                end = at + size
                i = at
                while i + 61 <= end:
                    base_offset, batch_len = struct.unpack_from(">qi", r, i)
                    stop = i + 12 + batch_len
                    # A response truncated at max_bytes ends mid-batch. The
                    # partial batch is not counted and not skipped past: the
                    # next fetch asks for it again from its base offset.
                    if stop > end:
                        break
                    if r[i + 16] != 2:
                        raise RuntimeError("record batch magic %d is not 2" % r[i + 16])
                    (attributes,) = struct.unpack_from(">h", r, i + 21)
                    if attributes & 0x07:
                        raise RuntimeError(
                            "record batch is compressed (attributes %d), so its record "
                            "lengths cannot be read without decompressing it" % attributes)
                    (last_delta,) = struct.unpack_from(">i", r, i + 23)
                    (declared,) = struct.unpack_from(">i", r, i + 57)
                    spans.append((i + 61, stop, declared))
                    offsets[partition] = base_offset + last_delta + 1
                    i = stop
                at = end
            # The broker's own statement of where the partition ends. Nothing
            # here infers a drained partition from a quiet one.
            if offsets[partition] >= high:
                drained = partition
    return spans, drained


def varint(r, i):
    shift = 0
    value = 0
    while True:
        b = r[i]
        i += 1
        value |= (b & 0x7F) << shift
        if b < 0x80:
            break
        shift += 7
    return (value >> 1) ^ -(value & 1), i


def count(r, spans):
    """Walks every record, for its payload length and for the count.

    The count is the reason this is not read from the batch header alone: the
    header's is the broker's claim and this is the bytes' own answer, and a
    disagreement means this program has misread the wire rather than that the
    broker served fewer records."""
    msgs = 0
    value_bytes = 0
    for at, stop, declared in spans:
        i = at
        counted = 0
        while i < stop:
            length, i = varint(r, i)
            after = i + length
            i += 1                      # attributes
            _, i = varint(r, i)         # timestamp delta
            _, i = varint(r, i)         # offset delta
            k, i = varint(r, i)         # key length, -1 when absent
            if k > 0:
                i += k
            v, i = varint(r, i)         # value length
            if v > 0:
                value_bytes += v
            counted += 1
            i = after
        if counted != declared:
            raise RuntimeError(
                "record batch declares %d records and %d were parsed" % (declared, counted))
        msgs += counted
    return msgs, value_bytes


def consumer(want, seconds, ready_w, go_r, out_w):
    try:
        broker = Broker()
        offsets = broker.offsets_at(want, -2)
        order = list(want)
        # Warm up outside the window: the connection, the offsets and the first
        # fetch are none of them what the broker is being measured on.
        broker.fetch(offsets, order)
        locate(broker.recv(), offsets)
        os.write(ready_w, b"R")
        if os.read(go_r, 1) != b"G":
            raise RuntimeError("no start signal")
        start = time.monotonic()
        msgs = 0
        value_bytes = 0
        broker.fetch(offsets, order)
        while True:
            r = broker.recv()
            order = order[1:] + order[:1]
            spans, drained = locate(r, offsets)
            if drained is not None:
                raise RuntimeError(
                    "DRAINED: partition %d ran out of backlog %.2fs into a %.2fs window"
                    % (drained, time.monotonic() - start, seconds))
            over = time.monotonic() - start >= seconds
            # The next fetch goes out before the records in hand are walked, so
            # the walk overlaps the broker's work instead of following it.
            if not over:
                broker.fetch(offsets, order)
            m, b = count(r, spans)
            msgs += m
            value_bytes += b
            if over:
                break
        os.write(out_w, ("OK %d %d %.6f\n"
                         % (msgs, value_bytes, time.monotonic() - start)).encode())
    except BaseException as e:
        try:
            os.write(out_w, ("ERR %s: %s\n"
                             % (type(e).__name__, " ".join(str(e).split())[:400])).encode())
        except BaseException:
            pass
    # Never a return: a forked child that fell out of this function would run
    # the parent's command loop and answer its own replies onto the pipe.
    os._exit(0)


def run(seconds, splits):
    ready_r, ready_w = os.pipe()
    go_r, go_w = os.pipe()
    out_r, out_w = os.pipe()
    kids = []
    for want in splits:
        pid = os.fork()
        if pid == 0:
            os.close(ready_r)
            os.close(go_w)
            os.close(out_r)
            consumer(want, seconds, ready_w, go_r, out_w)
        kids.append(pid)
    os.close(ready_w)
    os.close(go_r)
    os.close(out_w)
    # Every consumer starts together, so the window measures the parallelism it
    # names rather than a ragged ramp into it.
    ready = b""
    while len(ready) < len(splits):
        more = os.read(ready_r, len(splits) - len(ready))
        if not more:
            break
        ready += more
    if len(ready) == len(splits):
        os.write(go_w, b"G" * len(splits))
    lines = []
    pending = b""
    while len(lines) < len(splits):
        chunk = os.read(out_r, 4096)
        if not chunk:
            break
        pending += chunk
        while b"\n" in pending:
            line, pending = pending.split(b"\n", 1)
            lines.append(line.decode())
    for pid in kids:
        os.waitpid(pid, 0)
    for fd in (ready_r, go_w, out_r):
        os.close(fd)
    bad = [l for l in lines if not l.startswith("OK")]
    if bad:
        return "ERR " + bad[0][4:].strip()
    if len(lines) < len(splits):
        return "ERR only %d of %d consumers answered" % (len(lines), len(splits))
    msgs = sum(int(l.split()[1]) for l in lines)
    value_bytes = sum(int(l.split()[2]) for l in lines)
    # The longest consumer's window, not the mean: a consumer that finished
    # early contributed its messages to the numerator, and dividing by a shorter
    # denominator than the aggregate actually took would overstate the ceiling.
    elapsed = max(float(l.split()[3]) for l in lines)
    return "OK %d %d %.6f" % (msgs, value_bytes, elapsed)


try:
    probe = Broker()
    want = list(range(partitions))
    earliest = probe.offsets_at(want, -2)
    latest = probe.offsets_at(want, -1)
    reply("READY " + " ".join(str(latest[p] - earliest[p]) for p in want))
except BaseException as e:
    reply("ERR %s: %s" % (type(e).__name__, " ".join(str(e).split())[:400]))
    raise SystemExit(1)

while True:
    field = command()
    if not field or field[0] == "QUIT":
        break
    if field[0] == "RUN":
        reply(run(float(field[1]), [[int(p) for p in s.split(",")] for s in field[2:]]))
    else:
        reply("ERR unknown command %r" % (field[0],))
"##;

#[cfg(test)]
mod tests {
    use super::*;

    /// The split is the harness's decision and the container's instruction, so
    /// the line that carries it has to say exactly which consumer gets which
    /// partitions — a consumer with an empty list would read nothing and still
    /// contribute its window to the denominator.
    #[test]
    fn a_run_line_names_the_window_and_one_partition_list_per_consumer() {
        assert_eq!(
            format_run(
                &[vec![0, 2, 4, 6], vec![1, 3, 5, 7]],
                Duration::from_millis(660)
            ),
            "RUN 0.660 0,2,4,6 1,3,5,7\n"
        );
        assert_eq!(
            format_run(&[vec![0], vec![1]], Duration::from_secs(8)),
            "RUN 8.000 0 1\n"
        );
    }

    /// The window is sized against the shallowest partition, so the depths have
    /// to arrive per partition rather than as a total.
    #[test]
    fn a_ready_line_carries_one_backlog_per_partition() {
        assert_eq!(
            parse_ready("READY 187500 187500 187499").expect("a well-formed ready line"),
            vec![187_500, 187_500, 187_499]
        );
        assert!(
            parse_ready("READY").is_err(),
            "no partitions is not a topic"
        );
        assert!(parse_ready("ERR ConnectionRefusedError").is_err());
        assert!(parse_ready("READY 187500 many").is_err());
    }

    #[test]
    fn a_window_reply_carries_the_messages_the_bytes_and_the_time_they_took() {
        let fetched = parse_fetch("OK 1122334 4562291110 0.686000").expect("a well-formed reply");
        assert_eq!(fetched.msgs, 1_122_334);
        assert_eq!(fetched.bytes, 4_562_291_110);
        assert!((fetched.elapsed_s - 0.686).abs() < 1e-9);
    }

    /// The ported refusal. A partition that ran dry inside the window means the
    /// remainder of the window measured an idle broker, and the figure it would
    /// produce is the rate of a broker with nothing to serve.
    #[test]
    fn a_drained_partition_is_refused_by_name_rather_than_reported_as_a_lower_rate() {
        let e = parse_fetch(
            "ERR RuntimeError: DRAINED: partition 3 ran out of backlog 0.61s into a 0.80s window",
        )
        .expect_err("a drained window is a refusal");
        assert!(e.starts_with("REFUSED (DRAINED)"), "{e}");
        assert!(e.contains("partition 3"), "{e}");
    }

    /// A failure that is not about the corpus keeps the broker's own words: a
    /// misparsed record batch and a broker that closed the connection are
    /// different findings, and only one of them is about this program.
    #[test]
    fn a_fetcher_that_reports_an_error_fails_the_window_rather_than_reporting_what_it_read() {
        let e =
            parse_fetch("ERR RuntimeError: record batch declares 256 records and 255 were parsed")
                .expect_err("an ERR reply is a refusal");
        assert!(e.contains("declares 256 records"), "{e}");
        assert!(!e.starts_with("REFUSED (DRAINED)"), "{e}");
        assert!(parse_fetch("").is_err());
        assert!(parse_fetch("READY 187500").is_err());
        assert!(
            parse_fetch("OK 1").is_err(),
            "a truncated reply is not a window"
        );
        assert!(parse_fetch("OK many 1 1.0").is_err());
    }

    /// No messages over no seconds is arithmetic, not a measurement, and it is
    /// exactly what a window whose every fetch failed would otherwise report.
    #[test]
    fn a_reply_of_no_messages_or_no_window_is_refused_rather_than_divided_by() {
        assert!(parse_fetch("OK 0 0 0.8").is_err());
        assert!(parse_fetch("OK 100 406500 0.0").is_err());
    }
}
