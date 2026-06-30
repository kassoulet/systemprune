//! Regression smoke test for the GUI's "Application not responsive" freeze.
//!
//! Verifies that the production **bridge pattern** used by `do_delete`
//! and `do_scan` in `gui/src/window.rs` — `runtime.handle().spawn(...)`
//! for the worker + `glib::MainContext::spawn_local(...)` for the
//! main-thread effect — keeps the GTK main loop responsive while a
//! long-running async op is in flight on a tokio worker.
//!
//! # What breaks under the regression
//!
//! Before the bridge shipped, the GUI called
//! `runtime.block_on(orch.delete_many(...))` on the GTK main thread.  With a
//! `current_thread` tokio runtime, `block_on` drove the spawned task on the
//! calling thread, monopolising the GTK main loop for the entire
//! `delete_many` duration; after several seconds GTK fires its modal
//! "Application not responsive" dialog.
//!
//! The fix is the bridge.  This smoke test asserts the shape of that bridge
//! (`Handle::spawn(...)` → `tokio::sync::oneshot` → `MainContext::spawn_local(...)`)
//! does in fact keep the GTK main loop responsive while a "slow
//! `delete_many`" (modelled as a 500 ms `tokio::time::sleep`) runs on a
//! tokio worker.  The regression here is documented in:
//!
//! - `gui/src/window.rs` `do_delete` / `do_scan` doc blocks,
//! - `core/src/orchestrator.rs` `Orchestrator::scan_all` /
//!   `Orchestrator::delete_many` doc warnings.
//!
//! The test fails when:
//!
//!   1. **No idle event is ever dispatched** during the deadline window —
//!      i.e. the loop is frozen for the entire slow-op duration.  This is
//!      the canonical symptom of the original freeze.
//!   2. **The idle event only fires after the worker completes** — i.e.
//!      the loop catches up only after the await is gone, not during it.
//!      This is a milder variant of the same regression: the main thread
//!      is responsive in total but not *while* the await is in flight.
//!
//! # Scope: bridge-pattern shape test, not call-site regression test
//!
//! **This is a *bridge-pattern shape* test, not a `do_delete` regression
//! test.**  We do not invoke `do_delete` directly: the test reproduces the
//! pattern in a self-contained scenario with its own runtime.  A future
//! contributor who reintroduces `runtime.block_on(...)` into `do_delete`'s
//! body would not trip this test (the test exercises a different runtime).
//! The `// UI-thread safety:` doc blocks on the production handlers and
//! `Orchestrator::scan_all` / `Orchestrator::delete_many` carry the
//! call-site guarantee; this smoke test guards the *pattern*.  Both
//! layers together fence off both ends of a regression.
//!
//! # Glib source priority ordering (why the timing claim works)
//!
//! `MainContext::spawn_local` registers at `G_PRIORITY_DEFAULT`;
//! `glib::idle_add_once` registers at idle priority (lower).  When the
//! first `iteration(false)` runs:
//!
//!   1. The bridge future is polled, awaits `rx`, and yields (worker
//!      hasn't sent yet).
//!   2. The idle source fires (nothing higher-priority pending).
//!
//! So `pumped_at` reflects "after the bridge has registered and yielded at
//! least once", which is the property we want the test to assert.
//! Re-ordering the registration (e.g. moving the `idle_add_once` after
//! the bridge future registration) would still leave the idle source
//! firing at-or-after worker spawn — this comment is a guard against
//! future refactors that might inadvertently change the effective
//! ordering.
//!
//! # Running
//!
//! ```text
//! cargo test -p systemprune-gui --test ui_thread_safety
//! ```
//!
//! Headless CI environments need a working `$DISPLAY` (or `$WAYLAND_DISPLAY`)
//! for `gtk::init()` to succeed.  On a typical Linux CI runner, prefix the
//! test invocation with [`xvfb-run`](https://manpages.debian.org/xvfb-run):
//!
//! ```text
//! xvfb-run -a cargo test -p systemprune-gui --test ui_thread_safety
//! ```
//!
//! If `gtk::init()` fails, the test logs the failure to stderr and
//! returns early so the runner reports `PASS` rather than `FAIL` — the
//! test isn't worth holding up CI in a display-less runner, and the local
//! developer (or a headed CI lane) catches the freeze by running the test
//! interactively.

use glib::{idle_add_once, MainContext};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Once, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

/// Simulated `delete_many` duration.  Real `delete_many` shells out to
/// `docker container rm`, `conda env remove`, `go clean -cache`, ... which
/// can each take seconds.  We use a value large enough to make the
/// "during the wait" claim non-trivial, but small enough to keep the test
/// fast (total test time ≪ 1 s).
const SLOW_OP_MS: u64 = 500;

/// Maximum time the test will spin the main loop waiting for evidence
/// that it pumped events during the slow op.  A regression to
/// `block_on`-on-main-thread would freeze the loop here; this deadline
/// bounds the test so it fails deterministically with a useful message
/// instead of hanging.  Sized at 10× SLOW_OP_MS to absorb scheduler jitter
/// on slow runners.
const DEADLINE_MS: u64 = 5_000;

/// Slack added to the slow-op duration when checking assertion 2, so a
/// busy CI runner doesn't flake the test on a 51 ms run that nominally
/// should be 50 ms.  Pathological regressions (idle fires tens of
/// seconds after worker) still fail loudly under this bound.
const SLACK_MS: u64 = 50;

#[test]
fn main_loop_pumps_events_during_slow_async_op() {
    if !ensure_gtk_init() {
        // Logged by `ensure_gtk_init` already.  Treating absence
        // of display as "test not applicable" so headless CI lanes
        // don't have a hard failure on every run.
        return;
    }

    // --- (a) Idle source: flips a flag with a timestamp when the GTK
    //         main loop processes it. ---
    //
    // `idle_add_once` requires its closure to be `Send + 'static`.
    // `std::sync::OnceLock` (stable since Rust 1.70) is the
    // lightest weight primitive for "set-once by one writer,
    // read-once by one reader" — no `Mutex`, no `unwrap`, no
    // poison risk.  The idle callback is exactly that single
    // writer; the test reads once after the loop.
    let pumped: Arc<OnceLock<Instant>> = Arc::new(OnceLock::new());
    let pumped_for_idle = pumped.clone();
    idle_add_once(move || {
        // `set` returns `Result<(), Instant>` — only fails if
        // someone already wrote (we won't).  Ignored.
        let _ = pumped_for_idle.set(Instant::now());
    });

    // --- (b) Slow op: a 500 ms sleep on a tokio worker thread. ---
    //
    // We build a multi-thread runtime and park it on a dedicated OS
    // thread so the runtime stays alive for the duration of the test.
    // The actual slow op runs on one of the runtime's worker threads,
    // not on the main thread, mirroring the production
    // `state.runtime.handle().spawn(...)` call site in `do_delete`.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .expect("build tokio multi-thread runtime");
    let park_handle = rt.handle().clone();
    let park_thread = std::thread::spawn(move || {
        // Drive the runtime on this OS thread.  The
        // `pending().await` keeps it alive without consuming a
        // worker; the actual slow op is on a separate worker
        // spawned by `rt.handle().spawn(...)`.
        park_handle.block_on(async {
            std::future::pending::<()>().await;
        });
    });

    let (tx, rx) = oneshot::channel::<()>();
    let slow_started_at = Instant::now();
    rt.handle().spawn(async move {
        tokio::time::sleep(Duration::from_millis(SLOW_OP_MS)).await;
        let _ = tx.send(());
    });

    // --- (c) Bridge back to the GTK main thread (mirrors
    //         `do_delete`'s production bridge). ---
    //
    // `MainContext::default()` is the GLib main context that drives
    // the GTK main loop.  `.spawn_local(fut)` queues `fut` so it's
    // polled between iterations; the future runs only on the main
    // context, so `!Send`-ness is acceptable here.  When the
    // runtime hits `shutdown_timeout` later, the spawned
    // `tx.send(...)` task is cancelled, so `rx` is dropped and
    // this future completes with `Err(Canceled)` — harmless.
    MainContext::default().spawn_local(async move {
        let _ = rx.await;
    });

    // --- (d) Spin the main loop until either the idle callback
    //         fires OR the deadline elapses. ---
    //
    // `MainContext::iteration(false)` runs a single batch of
    // dispatch and returns.  Idle sources fire at the end of the
    // batch when nothing else is ready.  We deliberately avoid
    // `MainLoop::run()` so we keep control over the deadline —
    // a hung loop would otherwise manifest as a test timeout,
    // not the `DEADLINE_MS` failure we want.
    let main_ctx = MainContext::default();
    let deadline = Instant::now() + Duration::from_millis(DEADLINE_MS);
    while Instant::now() < deadline {
        main_ctx.iteration(false);
        if pumped.get().is_some() {
            break;
        }
    }

    // --- (e) Cleanup.  Tolerate an `Err` join — `block_on(pending())`
    //         is force-aborted when the runtime hits `shutdown_timeout`,
    //         which surfaces as a benign tokio-internal panic. ---
    rt.shutdown_timeout(Duration::from_millis(100));
    let _ = park_thread.join();

    // --- (f) Assertions. ---
    //
    // Assertion 1: the idle source fired at all within the
    // deadline window.  Catches canonical freeze.  `Instant` is
    // `Copy`, so `*&Instant` is a value, not a place.
    let pumped_at: Instant = *pumped.get().unwrap_or_else(|| {
        panic!(
            "GTK main loop did not pump idle callbacks during the \
             {DEADLINE_MS} ms window while a {SLOW_OP_MS} ms async op \
             was in flight. This is the 'Application not responsive' \
             regression: the GUI thread is blocked on \
             `runtime.block_on(...)` instead of dispatching to a \
             worker via `Handle::spawn(...)` + \
             `MainContext::spawn_local(...)`."
        )
    });

    // Assertion 2: the idle source fired well-before the slow op
    // completed.  The loop reaches `iteration(false)` immediately
    // after the bridge future is registered, and the idle source
    // is dispatched at end-of-batch — so `elapsed_at_pump` is
    // typically a handful of µs, well below `SLOW_OP_MS`.  We
    // permit 50 ms of slack for scheduler jitter on a loaded
    // CI runner; the assertion still fails loudly for any
    // pathological regression where idle fires after the
    // worker completes.
    let elapsed_at_pump = pumped_at.duration_since(slow_started_at);
    assert!(
        elapsed_at_pump < Duration::from_millis(SLOW_OP_MS + SLACK_MS),
        "Idle callback fired at {elapsed_at_pump:?} relative to \
         slow-op start, AFTER the {SLOW_OP_MS} ms async op \
         (+ {SLACK_MS} ms slack) completed. The main loop \
         caught up only after the worker finished, not during \
         the wait. The property under test is that the main \
         loop pumps events WHILE the worker is in flight."
    );
}

/// Initialise GTK once per test process, returning whether init
/// succeeded.  GTK init is process-wide and may only be called once;
/// subsequent calls would panic.  We gate it on a `Once` so multiple
/// tests in this binary cooperate.
///
/// On failure (typically: no `$DISPLAY` in CI), we log to stderr and
/// return `false` so the caller can early-return without failing.  A
/// headed CI lane (or local developer) will still observe the freeze
/// assertion when running interactively.
fn ensure_gtk_init() -> bool {
    static ONCE: Once = Once::new();
    static INIT_OK: AtomicBool = AtomicBool::new(false);
    ONCE.call_once(|| match gtk::init() {
        Ok(()) => {
            INIT_OK.store(true, Ordering::Relaxed);
        }
        Err(e) => {
            eprintln!(
                "[ui_thread_safety] gtk::init() failed ({e}); \
                     skipping. Headless CI may need \
                     `xvfb-run -a cargo test ...` for this test."
            );
        }
    });
    INIT_OK.load(Ordering::Relaxed)
}
