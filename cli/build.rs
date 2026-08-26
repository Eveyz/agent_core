fn main() {
    let hash = git_stdout(&["rev-parse", "--short=12", "HEAD"]).unwrap_or_else(|| "unknown".into());
    let date =
        git_stdout(&["show", "-s", "--format=%cs", "HEAD"]).unwrap_or_else(|| "unknown".into());
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "unknown".into());

    println!("cargo:rustc-env=GIT_COMMIT_HASH={hash}");
    println!("cargo:rustc-env=GIT_COMMIT_DATE={date}");
    println!("cargo:rustc-env=BUILD_PROFILE={profile}");
    println!("cargo:rerun-if-changed=../.git/HEAD");
    println!("cargo:rerun-if-changed=../.git/refs/heads");
}

fn git_stdout(args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8(output.stdout).ok()?;
    let trimmed = s.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}
