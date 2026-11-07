use std::process::Command;

fn version(flag: &str) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_ob-node")).arg(flag).output().unwrap();
    assert!(output.status.success(), "{flag}: {output:?}");
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn version_flag_prints_the_version_and_commit() {
    let line = version("--version");
    let expected = format!("ob-node {} ({}", env!("CARGO_PKG_VERSION"), env!("OB_BUILD_COMMIT"));
    assert!(line.starts_with(&expected), "{line:?} should start with {expected:?}");
    assert!(line.contains(&format!("{} build", env!("OB_BUILD_KIND"))), "{line:?}");
    assert!(line.contains(env!("OB_BUILD_DATE")), "{line:?}");
    assert_eq!(line.contains("-dirty"), env!("OB_BUILD_DIRTY") == "true", "{line:?}");
    assert_eq!(line.lines().count(), 1);
    assert_eq!(version("-V"), line);
    assert_eq!(version("version"), line);
}
