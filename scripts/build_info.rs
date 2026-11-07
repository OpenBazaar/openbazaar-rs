// Build metadata for ob-node and the desktop shell. Both build scripts
// `include!` this file so the two binaries describe a build the same way.
//
// It sets these compile-time variables (read with `env!`):
//
//   OB_BUILD_COMMIT  first 7 hex digits of HEAD, or "unknown" without git
//   OB_BUILD_DIRTY   "true" when tracked files differed from HEAD
//   OB_BUILD_DATE    UTC, 2026-01-31T12:00:00Z
//   OB_BUILD_KIND    "release", "ci", or "dev"
//
// Each can be set in the environment instead, which is how the release
// workflow records the tree's state before it stamps the version. Without
// overrides the kind is "ci" under GitHub Actions ("release" for a tag build)
// and "dev" everywhere else. SOURCE_DATE_EPOCH fixes the date.
//
// The script reruns when HEAD moves, the index changes, or `src` changes, so
// an edit outside this package can leave a stale dirty flag until then.
mod build_info {
    use std::env;
    use std::path::PathBuf;
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    const OVERRIDES: [&str; 4] = ["OB_BUILD_COMMIT", "OB_BUILD_DIRTY", "OB_BUILD_DATE", "OB_BUILD_KIND"];

    pub fn emit() {
        for name in OVERRIDES.iter().chain(&["SOURCE_DATE_EPOCH", "GITHUB_ACTIONS", "GITHUB_REF_TYPE"]) {
            println!("cargo:rerun-if-env-changed={name}");
        }
        println!("cargo:rerun-if-changed=src");
        if let Some(dir) = git(&["rev-parse", "--absolute-git-dir"]) {
            let dir = PathBuf::from(dir);
            for file in ["HEAD", "logs/HEAD", "index"] {
                println!("cargo:rerun-if-changed={}", dir.join(file).display());
            }
        }
        set("OB_BUILD_COMMIT", commit());
        set("OB_BUILD_DIRTY", dirty().to_string());
        set("OB_BUILD_DATE", date());
        set("OB_BUILD_KIND", kind());
    }

    fn set(name: &str, value: String) {
        println!("cargo:rustc-env={name}={value}");
    }

    fn git(args: &[&str]) -> Option<String> {
        // --no-optional-locks keeps `git status` from rewriting the index,
        // which would make this script rerun on every build.
        let output = Command::new("git").arg("--no-optional-locks").args(args).output().ok()?;
        output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    fn overridden(name: &str) -> Option<String> {
        env::var(name).ok().filter(|value| !value.is_empty())
    }

    fn commit() -> String {
        let commit = overridden("OB_BUILD_COMMIT").or_else(|| git(&["rev-parse", "HEAD"])).unwrap_or_default();
        if commit.len() >= 7 && commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            commit[..7].to_ascii_lowercase()
        } else {
            "unknown".into()
        }
    }

    fn dirty() -> bool {
        match overridden("OB_BUILD_DIRTY").as_deref() {
            Some("true") => true,
            Some("false") => false,
            Some(other) => panic!("OB_BUILD_DIRTY must be true or false, not {other:?}"),
            None => git(&["status", "--porcelain", "--untracked-files=no"]).is_some_and(|status| !status.is_empty()),
        }
    }

    fn kind() -> String {
        match overridden("OB_BUILD_KIND") {
            Some(kind) if ["release", "ci", "dev"].contains(&kind.as_str()) => kind,
            Some(other) => panic!("OB_BUILD_KIND must be release, ci, or dev, not {other:?}"),
            None if env::var("GITHUB_ACTIONS").as_deref() == Ok("true") => {
                if env::var("GITHUB_REF_TYPE").as_deref() == Ok("tag") { "release".into() } else { "ci".into() }
            }
            None => "dev".into(),
        }
    }

    fn date() -> String {
        if let Some(date) = overridden("OB_BUILD_DATE") {
            return date;
        }
        let secs = overridden("SOURCE_DATE_EPOCH")
            .map(|epoch| epoch.parse().expect("SOURCE_DATE_EPOCH must be seconds since 1970"))
            .unwrap_or_else(|| SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0));
        let (days, rest) = ((secs / 86_400) as i64, secs % 86_400);
        // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = doy - (153 * mp + 2) / 5 + 1;
        let month = if mp < 10 { mp + 3 } else { mp - 9 };
        let year = yoe + era * 400 + i64::from(month <= 2);
        format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", rest / 3_600, rest % 3_600 / 60, rest % 60)
    }
}
