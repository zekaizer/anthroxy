//! Records the git state the binary was built from, so `--version` can tell a
//! release from a locally modified tree.

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn main() {
    let describe = match git(&["rev-parse", "--short=7", "HEAD"]) {
        Some(hash) => {
            let dirty = git(&["status", "--porcelain", "--untracked-files=no"])
                .is_some_and(|s| !s.is_empty());
            if dirty { format!("{hash}-dirty") } else { hash }
        }
        None => "unknown".to_owned(),
    };
    println!("cargo:rustc-env=ANTHROXY_GIT={describe}");
    // Re-run when the checked-out commit or the index changes.
    for path in [".git/HEAD", ".git/index"] {
        println!("cargo:rerun-if-changed={path}");
    }
    if let Some(head) = std::fs::read_to_string(".git/HEAD")
        .ok()
        .and_then(|h| h.strip_prefix("ref: ").map(|r| r.trim().to_owned()))
    {
        println!("cargo:rerun-if-changed=.git/{head}");
    }
}
