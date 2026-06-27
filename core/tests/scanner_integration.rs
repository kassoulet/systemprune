//! End-to-end integration tests for each built-in scanner.
//!
//! Exercises the scan → parse → classify path against scripted
//! "engine" binaries so we can detect regressions like the
//! Podman `{{.ID}}` vs `{{.ImageID}}` bug and the lost malformed-line
//! resilience without depending on Docker / Podman / etc. being
//! installed.
//!
//! # Test isolation
//!
//! Every test mutates the process-level ``PATH`` environment
//! variable and then runs a subprocess that consults ``PATH`` to
//! find the fake engine binary. Cargo's default worker pool runs
//! tests in parallel, so without serialisation test B's
//! ``set_var(\"PATH\", …)`` would race with test A's subprocess
//! lookup. Each test therefore acquires :data:`PATH_LOCK` at the
//! top and holds it until the end of its body.

use std::collections::HashSet;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::Mutex;
use systemprune_core::models::{Category, Engine as CoreEngine, PrunableItem, Status};
use systemprune_core::scanners::base::BaseScanner;
use systemprune_core::scanners::Scanner;
use tempfile::TempDir;

/// Serialises tests that mutate the process ``PATH``.
static PATH_LOCK: Mutex<()> = Mutex::new(());

fn make_script(dir: &std::path::Path, name: &str, body: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .mode(0o755)
        .open(&p)
        .unwrap();
    std::fs::write(&p, body).unwrap();
    p
}

/// Replace ``PATH`` with *dir* prepended. Caller **must** already
/// hold :data:`PATH_LOCK`.
fn set_path_locked(dir: &std::path::Path) {
    let current = std::env::var("PATH").unwrap_or_default();
    let next = format!(
        "{}:{}",
        dir.to_string_lossy(),
        if current.is_empty() {
            String::new()
        } else {
            current
        }
    );
    std::env::set_var("PATH", &next);
}

fn find_scanner(source: &str) -> std::sync::Arc<dyn Scanner> {
    systemprune_core::scanners::all_scanners()
        .into_iter()
        .find(|s| s.source() == source)
        .unwrap_or_else(|| panic!("missing scanner for source={}", source))
}

// ---------------------------------------------------------------------------
// Podman: regression test for the {{.ImageID}} bug.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn podman_marks_in_use_image_as_active() {
    let _guard = PATH_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();
    make_script(
        tmp.path(),
        "podman",
        r#"#!/bin/sh
case "$1" in
  images)
    printf '[{"Id":"img-in-use:abcdef012345","Names":["myapp"],"Size":"142 MB","Repository":"myapp","Tag":"v1"},{"Id":"img-orphan:fedcba987654","Names":["unused"],"Size":"5 MB","Repository":"unused","Tag":"latest"}]'
    ;;
  ps)
    echo "myapp    img-in-use:abcdef012345"
    ;;
esac
"#,
    );
    set_path_locked(tmp.path());
    let scanner = find_scanner("podman");
    let items = scanner.get_items().await.unwrap();
    let by_id: std::collections::HashMap<_, _> =
        items.iter().map(|i| (i.id.clone(), i)).collect();

    assert_eq!(
        by_id["img-in-use:abcdef012345"].status,
        Status::Active,
        "the in-use image must be detected via the ImageID template"
    );
    assert_eq!(by_id["img-orphan:fedcba987654"].status, Status::Unused);
}

// ---------------------------------------------------------------------------
// Docker: malformed JSON lines must be skipped without aborting the scan.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn docker_skips_malformed_image_json_lines() {
    let _guard = PATH_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();
    make_script(
        tmp.path(),
        "docker",
        r#"#!/bin/sh
case "$1" in
  images)
    printf '%s\n%s\n%s\n' \
      '{"ID":"sha256:abc123dead","Repository":"nginx","Tag":"latest","Size":"142 MB","CreatedSince":"2 days ago","CreatedAt":"2024"}' \
      'not json at all' \
      '{"ID":"sha256:fedcba","Repository":"<none>","Tag":"<none>","Size":"0B","CreatedSince":"","CreatedAt":""}'
    ;;
  ps)
    if [ "$2" = "-a" ]; then
      printf '{"ID":"cnt1","Names":"web","State":"exited","Image":"nginx:latest","Size":"0B"}\n'
    else
      printf 'nginx:latest sha256:abc123dead\n'
    fi
    ;;
esac
"#,
    );
    set_path_locked(tmp.path());
    let scanner = find_scanner("docker");
    let items = scanner.get_items().await.unwrap();
    let by_id: std::collections::HashMap<_, _> =
        items.iter().map(|i| (i.id.clone(), i)).collect();

    assert!(!by_id.contains_key("not json at all"));
    assert_eq!(by_id["sha256:abc123dead"].status, Status::Active);
    assert_eq!(by_id["sha256:fedcba"].status, Status::Dangling);
}

#[tokio::test]
async fn docker_keeps_exited_containers() {
    let _guard = PATH_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();
    make_script(
        tmp.path(),
        "docker",
        r#"#!/bin/sh
if [ "$1" = "ps" ]; then
  printf '{"ID":"cnt1","Names":"web","State":"exited","Image":"nginx:latest","Size":"0B"}\n'
elif [ "$1" = "images" ]; then
  echo ""
fi
"#,
    );
    set_path_locked(tmp.path());
    let scanner = find_scanner("docker");
    let items = scanner.get_items().await.unwrap();
    let containers: Vec<_> =
        items.iter().filter(|i| i.category == Category::Container).collect();
    assert_eq!(containers.len(), 1);
    assert_eq!(containers[0].status, Status::Stopped);
}

// ---------------------------------------------------------------------------
// Flatpak: column parsing + active-app detection.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn flatpak_marks_running_app_as_active() {
    let _guard = PATH_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();
    make_script(
        tmp.path(),
        "flatpak",
        r#"#!/bin/sh
if echo "$@" | grep -q '\-\-app'; then
  printf 'Application ID   Size   Runtime\n'
  printf 'org.gimp.GIMP    142.1 MB   org.gnome.Platform/x86_64/45\n'
else
  printf 'Application   Size   Runtime   Arch   Branch\n'
  printf 'org.gnome.Platform   1.2 GB   master   x86_64   45\n'
fi
if [ "$1" = "ps" ]; then
  echo "Application"
  echo "org.gimp.GIMP"
fi
"#,
    );
    set_path_locked(tmp.path());
    let scanner = find_scanner("flatpak");
    let items = scanner.get_items().await.unwrap();
    let by_id: std::collections::HashMap<_, _> =
        items.iter().map(|i| (i.id.clone(), i)).collect();

    assert_eq!(by_id["org.gimp.GIMP"].status, Status::Active);
    assert_eq!(by_id["org.gimp.GIMP"].category, Category::App);
    assert_eq!(by_id["org.gnome.Platform"].status, Status::Unused);
    assert_eq!(by_id["org.gnome.Platform"].category, Category::Runtime);
}

// ---------------------------------------------------------------------------
// Snap: protected snaps are filtered out and active services are detected.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn snap_filters_protected_and_marks_running_service() {
    let _guard = PATH_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();
    make_script(
        tmp.path(),
        "snap",
        r#"#!/bin/sh
if [ "$1" = "list" ]; then
  printf 'Name   Version   Rev   Size\n'
  printf 'firefox   1234   4567   250 MB\n'
  printf 'snapd   2.61   21184   30 MB\n'
elif [ "$1" = "services" ]; then
  printf 'Service   Startup   Current\n'
  printf 'snap.firefox.daemon   enabled   active\n'
fi
"#,
    );
    set_path_locked(tmp.path());
    let scanner = find_scanner("snap");
    let items = scanner.get_items().await.unwrap();

    let names: HashSet<&str> = items.iter().map(|i| i.id.as_str()).collect();
    assert!(names.contains("firefox"));
    assert!(!names.contains("snapd"), "snapd is protected and must be omitted");

    let firefox = items.iter().find(|i| i.id == "firefox").unwrap();
    assert_eq!(firefox.status, Status::Active);

    let result = scanner
        .delete_item(&PrunableItem {
            id: "snapd".into(),
            name: "snapd".into(),
            engine: CoreEngine::Snap,
            source: "snap".into(),
            category: Category::SnapRevision,
            size_bytes: 0,
            status: Status::Unused,
            extra: Default::default(),
        })
        .await;
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// Conda: env list parsing, base-env skip, stale-path skip, and remove.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn conda_lists_and_removes_envs() {
    let _guard = PATH_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();

    // Create two real env directories with a file each so
    // `dir_size` returns a non-zero value.  Also leave a
    // `staleenv` path in the fake output that points at a
    // directory we never create, to exercise the stale-path
    // skip in `get_items`.
    let env_a = tmp.path().join("envs").join("myenv");
    let env_b = tmp.path().join("envs").join("otherenv");
    let stale_env = tmp.path().join("envs").join("staleenv");
    for env in [&env_a, &env_b] {
        std::fs::create_dir_all(env).unwrap();
        std::fs::write(env.join("file.txt"), "x").unwrap();
    }
    // `base` is intentionally NOT created: the scanner must
    // skip it for two reasons (name == "base" AND path missing).
    let base_path = tmp.path().join("base_missing");

    // The fake `conda` script.  It echoes a hard-coded env list
    // for `conda env list` (base + 3 named envs) and writes a
    // marker file + exits 0 for `conda env remove -p ... -y`.
    // The paths are baked in via `format!` so the script can
    // be written verbatim by `make_script`.
    let marker = tmp.path().join("remove_called");
    let script = format!(
        r#"#!/bin/sh
case "$1 $2" in
  "env list")
    printf '# conda environments:\n#\n'
    printf 'base {base}\n'
    printf 'myenv {a}\n'
    printf 'otherenv {b}\n'
    printf 'staleenv {stale}\n'
    ;;
  "env remove")
    : >> {marker}
    exit 0
    ;;
esac
"#,
        base = base_path.display(),
        a = env_a.display(),
        b = env_b.display(),
        stale = stale_env.display(),
        marker = marker.display(),
    );
    make_script(tmp.path(), "conda", &script);
    set_path_locked(tmp.path());

    let scanner = find_scanner("conda");
    let items = scanner.get_items().await.unwrap();

    // Four envs listed (base + 3), but `base` is skipped by
    // name and `staleenv` is skipped because its path does not
    // exist.  Two items must remain.
    assert_eq!(
        items.len(),
        2,
        "base env must be skipped by name and staleenv must be skipped by missing path"
    );

    let by_id: std::collections::HashMap<_, _> =
        items.iter().map(|i| (i.id.clone(), i)).collect();
    let myenv = by_id
        .get(&env_a.display().to_string())
        .expect("myenv must be present");
    let otherenv = by_id
        .get(&env_b.display().to_string())
        .expect("otherenv must be present");
    assert!(
        !by_id.contains_key(&base_path.display().to_string()),
        "base env must not be reported"
    );
    assert!(
        !by_id.contains_key(&stale_env.display().to_string()),
        "stale env (missing path) must not be reported"
    );

    // Per-item metadata for myenv.
    assert_eq!(myenv.name, "myenv");
    assert_eq!(myenv.source, "conda");
    assert_eq!(myenv.engine, CoreEngine::Conda);
    assert_eq!(myenv.category, Category::PythonVenv);
    assert_eq!(myenv.status, Status::Unused);
    assert!(
        myenv.size_bytes > 0,
        "size should be computed from the env directory (got {})",
        myenv.size_bytes
    );
    assert_eq!(myenv.extra.get("env_name"), Some(&"myenv".to_string()));
    assert_eq!(myenv.extra.get("path"), Some(&env_a.display().to_string()));

    // otherenv has the same shape.
    assert_eq!(otherenv.name, "otherenv");
    assert_eq!(otherenv.category, Category::PythonVenv);
    assert!(otherenv.size_bytes > 0);

    // `delete_item` must invoke `conda env remove -p <path> -y`
    // and succeed when the fake binary exits 0.  The marker
    // file is the proof that the script was actually called
    // with the expected arguments.
    let result = scanner.delete_item(myenv).await;
    assert!(result.is_ok(), "delete_item should succeed: {:?}", result);
    assert!(
        marker.exists(),
        "remove script should have been called by delete_item"
    );
}

// ---------------------------------------------------------------------------
// Go build cache: env GOCACHE parsing, size, and `go clean -cache`.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn go_cache_reports_and_cleans_via_fake_go_binary() {
    let _guard = PATH_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();

    // Create the fake build-cache directory with a file so
    // `dir_size` returns a non-zero value.  The
    // "non-existent cache dir" branch is exercised in its
    // own sub-test below.
    let cache_dir = tmp.path().join("go-build");
    std::fs::create_dir_all(&cache_dir).unwrap();
    std::fs::write(cache_dir.join("entry.txt"), "x").unwrap();

    // The fake `go` script.  For `go env GOCACHE` it echoes
    // the cache path on stdout (real `go` writes a single
    // trailing-newline-terminated line).  For `go clean
    // -cache` it touches a marker file and exits 0.
    let marker = tmp.path().join("clean_called");
    let script = format!(
        r#"#!/bin/sh
case "$1 $2" in
  "env GOCACHE")
    printf '{cache}\n'
    ;;
  "clean -cache")
    : >> {marker}
    exit 0
    ;;
esac
"#,
        cache = cache_dir.display(),
        marker = marker.display(),
    );
    make_script(tmp.path(), "go", &script);
    set_path_locked(tmp.path());

    let scanner = find_scanner("go_cache");
    let items = scanner.get_items().await.unwrap();

    assert_eq!(
        items.len(),
        1,
        "the fake `go env GOCACHE` should produce exactly one item"
    );
    let item = &items[0];
    assert_eq!(item.id, cache_dir.display().to_string());
    assert_eq!(item.name, "Go build cache");
    assert_eq!(item.source, "go_cache");
    assert_eq!(item.engine, CoreEngine::GoCache);
    assert_eq!(item.category, Category::BuildCache);
    assert_eq!(item.status, Status::Unused);
    assert!(
        item.size_bytes > 0,
        "size should be computed from the cache directory (got {})",
        item.size_bytes
    );
    assert_eq!(
        item.extra.get("path"),
        Some(&cache_dir.display().to_string())
    );

    // `delete_item` must invoke `go clean -cache` and
    // succeed when the fake binary exits 0.  The marker
    // file is the proof that the script was actually called.
    let result = scanner.delete_item(item).await;
    assert!(result.is_ok(), "delete_item should succeed: {:?}", result);
    assert!(
        marker.exists(),
        "go clean -cache should have been called by delete_item"
    );
}

#[tokio::test]
async fn go_cache_skips_when_gocache_path_missing() {
    let _guard = PATH_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();

    // The fake `go` reports a cache path that does NOT
    // exist on disk.  The scanner must treat this as "no
    // cache yet" and return an empty list, not an error.
    let missing = tmp.path().join("no-cache-here");
    let script = format!(
        r#"#!/bin/sh
if [ "$1 $2" = "env GOCACHE" ]; then
  printf '{p}\n'
fi
"#,
        p = missing.display(),
    );
    make_script(tmp.path(), "go", &script);
    set_path_locked(tmp.path());

    let scanner = find_scanner("go_cache");
    let items = scanner.get_items().await.unwrap();
    assert!(
        items.is_empty(),
        "non-existent GOCACHE path must produce no items, got {items:?}"
    );
}

#[tokio::test]
async fn go_cache_skips_when_gocache_stdout_is_empty() {
    let _guard = PATH_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();

    // The fake `go` returns an empty line for `go env
    // GOCACHE`.  The scanner must not panic on the empty
    // path and must return an empty list.
    make_script(
        tmp.path(),
        "go",
        "#!/bin/sh\n[ \"$1 $2\" = \"env GOCACHE\" ] || exit 0\n",
    );
    set_path_locked(tmp.path());

    let scanner = find_scanner("go_cache");
    let items = scanner.get_items().await.unwrap();
    assert!(
        items.is_empty(),
        "empty GOCACHE stdout must produce no items, got {items:?}"
    );
}

// ---------------------------------------------------------------------------
// ``BaseScanner::run`` propagates stderr on non-zero exit codes.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn base_scanner_run_returns_stderr_on_failure() {
    let _guard = PATH_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();
    make_script(
        tmp.path(),
        "failbin",
        "#!/bin/sh\necho intentional failure 1>&2\nexit 7\n",
    );
    set_path_locked(tmp.path());
    let base = BaseScanner::new("failbin", CoreEngine::Docker, "failbin");
    let err = base
        .run(&["failbin"], 5)
        .await
        .expect_err("failbin exits 7");
    assert_eq!(err.returncode, Some(7));
    assert!(err.stderr.contains("intentional failure"));
    assert_eq!(err.engine, "failbin");
}
