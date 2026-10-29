use std::process::Command;

fn main() {
    // Commit and its date for the "Informazioni" dialog; absent outside a git checkout.
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
    };
    if let Some(commit) = git(&["rev-parse", "--short", "HEAD"]) {
        println!("cargo:rustc-env=GF_COMMIT={commit}");
    }
    if let Some(date) = git(&["log", "-1", "--format=%cs"]) {
        println!("cargo:rustc-env=GF_COMMIT_DATE={date}");
    }
    // A new commit moves HEAD or the branch ref (not the index: no rebuild on `git add`).
    println!("cargo:rerun-if-changed=../.git/HEAD");
    println!("cargo:rerun-if-changed=../.git/refs/heads");
    tauri_build::build()
}
