use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;

use super::status::{Check, Expected, Verdict, collect};
use super::systemd::{CommandOutput, CommandRunner, ServiceError, command_line, render_unit};

/// Scripted `systemctl`/`loginctl`: keyed by the full command line.
#[derive(Default)]
struct FakeRunner {
    outputs: HashMap<String, CommandOutput>,
    calls: RefCell<Vec<String>>,
}

impl FakeRunner {
    fn with(mut self, command: &str, success: bool, stdout: &str, stderr: &str) -> Self {
        self.outputs.insert(
            command.to_owned(),
            CommandOutput {
                success,
                stdout: stdout.to_owned(),
                stderr: stderr.to_owned(),
            },
        );
        self
    }

    fn healthy() -> Self {
        Self::default()
            .with("systemctl --user is-system-running", true, "running", "")
            .with(
                "systemctl --user is-enabled anthroxy.service",
                true,
                "enabled",
                "",
            )
            .with(
                "systemctl --user is-active anthroxy.service",
                true,
                "active",
                "",
            )
            .with(
                "loginctl show-user luke --property=Linger --value",
                true,
                "yes",
                "",
            )
    }
}

impl CommandRunner for FakeRunner {
    fn run(&self, program: &str, args: &[&str]) -> Result<CommandOutput, ServiceError> {
        let command = command_line(program, args);
        self.calls.borrow_mut().push(command.clone());
        self.outputs
            .get(&command)
            .cloned()
            .ok_or_else(|| ServiceError::Command {
                command,
                detail: "No such file or directory".to_owned(),
            })
    }
}

fn verdict<'a>(checks: &'a [Check], name: &str) -> &'a Check {
    checks
        .iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("no check named {name}: {checks:?}"))
}

struct Fixture {
    _dir: tempfile::TempDir,
    exe: std::path::PathBuf,
    config: std::path::PathBuf,
    unit: std::path::PathBuf,
}

fn fixture(write_unit_for: Option<(&Path, &Path)>) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("bin").join("anthroxy");
    let config = dir.path().join("config.toml");
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::write(&exe, "").unwrap();
    std::fs::write(&config, "").unwrap();
    let unit = dir.path().join("anthroxy.service");
    if let Some((unit_exe, unit_config)) = write_unit_for {
        std::fs::write(&unit, render_unit(unit_exe, unit_config)).unwrap();
    }
    Fixture {
        _dir: dir,
        exe,
        config,
        unit,
    }
}

fn expected<'a>(f: &'a Fixture, health_url: Result<String, String>) -> Expected<'a> {
    Expected {
        exe: &f.exe,
        config: &f.config,
        unit_path: &f.unit,
        user: "luke",
        health_url,
    }
}

async fn health_listener() -> String {
    let app = axum::Router::new().route(
        "/healthz",
        axum::routing::get(|| async { "{\"status\":\"ok\"}" }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/healthz", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

#[tokio::test]
async fn fully_installed_system_is_all_ok() {
    let f = fixture(None);
    let f = {
        std::fs::write(&f.unit, render_unit(&f.exe, &f.config)).unwrap();
        f
    };
    let url = health_listener().await;
    let checks = collect(&expected(&f, Ok(url)), &FakeRunner::healthy()).await;
    for name in [
        "systemd",
        "unit file",
        "unit paths",
        "enabled",
        "active",
        "linger",
        "health",
    ] {
        let c = verdict(&checks, name);
        assert_eq!(c.verdict, Verdict::Ok, "{name}: {}", c.detail);
    }
    assert_eq!(checks.len(), 7);
}

#[tokio::test]
async fn missing_unit_skips_dependent_checks() {
    let f = fixture(None);
    let checks = collect(
        &expected(&f, Err("no config".into())),
        &FakeRunner::healthy(),
    )
    .await;
    let unit = verdict(&checks, "unit file");
    assert_eq!(unit.verdict, Verdict::Fail);
    assert!(unit.detail.contains("service install"), "{}", unit.detail);
    assert_eq!(verdict(&checks, "unit paths").verdict, Verdict::Skip);
    let health = verdict(&checks, "health");
    assert_eq!(health.verdict, Verdict::Skip);
    assert!(health.detail.contains("no config"));
}

#[tokio::test]
async fn stale_unit_paths_are_reported_with_both_sides() {
    let f = fixture(Some((
        Path::new("/old/anthroxy"),
        Path::new("/old/config.toml"),
    )));
    let checks = collect(&expected(&f, Err(String::new())), &FakeRunner::healthy()).await;
    let paths = verdict(&checks, "unit paths");
    assert_eq!(paths.verdict, Verdict::Fail);
    assert!(paths.detail.contains("/old/anthroxy"), "{}", paths.detail);
    assert!(
        paths.detail.contains(f.exe.to_str().unwrap()),
        "{}",
        paths.detail
    );
    assert!(paths.detail.contains("service install"), "{}", paths.detail);
}

#[tokio::test]
async fn systemd_unavailable_skips_systemctl_checks_with_hint() {
    let f = fixture(Some((Path::new("/x"), Path::new("/y"))));
    let runner = FakeRunner::default()
        .with(
            "systemctl --user is-system-running",
            false,
            "",
            "Failed to connect to bus: No medium found",
        )
        .with(
            "loginctl show-user luke --property=Linger --value",
            true,
            "no",
            "",
        );
    let checks = collect(&expected(&f, Err(String::new())), &runner).await;
    let systemd = verdict(&checks, "systemd");
    assert_eq!(systemd.verdict, Verdict::Fail);
    assert!(
        systemd.detail.contains("No medium found") && systemd.detail.contains("wsl.conf"),
        "{}",
        systemd.detail
    );
    assert_eq!(verdict(&checks, "enabled").verdict, Verdict::Skip);
    assert_eq!(verdict(&checks, "active").verdict, Verdict::Skip);
    let linger = verdict(&checks, "linger");
    assert_eq!(linger.verdict, Verdict::Fail);
    assert!(linger.detail.contains("enable-linger"), "{}", linger.detail);
    assert!(
        !runner
            .calls
            .borrow()
            .iter()
            .any(|c| c.contains("is-active")),
        "must not ask a dead manager"
    );
}

#[tokio::test]
async fn inactive_unit_points_at_the_journal() {
    let f = fixture(Some((Path::new("/x"), Path::new("/y"))));
    let runner = FakeRunner::healthy()
        .with(
            "systemctl --user is-active anthroxy.service",
            false,
            "failed",
            "",
        )
        .with(
            "systemctl --user is-enabled anthroxy.service",
            false,
            "disabled",
            "",
        );
    let checks = collect(
        &expected(&f, Ok("http://127.0.0.1:1/healthz".into())),
        &runner,
    )
    .await;
    let active = verdict(&checks, "active");
    assert_eq!(active.verdict, Verdict::Fail);
    assert!(
        active.detail.contains("failed") && active.detail.contains("journalctl"),
        "{}",
        active.detail
    );
    assert_eq!(verdict(&checks, "enabled").verdict, Verdict::Fail);
    let health = verdict(&checks, "health");
    assert_eq!(health.verdict, Verdict::Fail);
    assert!(health.detail.contains("127.0.0.1:1"), "{}", health.detail);
}

#[tokio::test]
async fn degraded_manager_still_counts_as_up() {
    let f = fixture(Some((Path::new("/x"), Path::new("/y"))));
    let runner =
        FakeRunner::healthy().with("systemctl --user is-system-running", false, "degraded", "");
    let checks = collect(&expected(&f, Err(String::new())), &runner).await;
    assert_eq!(verdict(&checks, "systemd").verdict, Verdict::Ok);
    assert_eq!(verdict(&checks, "active").verdict, Verdict::Ok);
}
