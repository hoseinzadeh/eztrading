//! Validation of SML source with the real transpiler plus a few structural lints.

use regex::Regex;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Result of validating one strategy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub ok: bool,
    pub diagnostics: Vec<String>,
}

impl Report {
    pub fn ok() -> Self {
        Self { ok: true, diagnostics: vec![] }
    }

    pub fn failed(msg: impl Into<String>) -> Self {
        Self { ok: false, diagnostics: vec![msg.into()] }
    }

    /// Text handed back to the model (and printed by `ezt-agent check`).
    pub fn render(&self) -> String {
        if self.ok {
            "OK: the compiler accepted this strategy.".to_owned()
        } else {
            format!("FAILED:\n{}", self.diagnostics.join("\n"))
        }
    }
}

/// Anything that can judge a strategy. The agent only talks to this trait, so
/// tests can use a fake and the CLI uses [`Checker`].
pub trait Validator {
    fn check(&self, source: &str) -> Report;
}

/// Runs the `compile` binary on a private copy of the standard library, then
/// applies [`lint`].
pub struct Checker {
    compiler: PathBuf,
    stdlib: PathBuf,
    timeout: Duration,
}

impl Checker {
    pub fn new(compiler: PathBuf, stdlib: PathBuf) -> Self {
        Self { compiler, stdlib, timeout: Duration::from_secs(30) }
    }

    /// Finds the compiler: explicit path, then next to this executable, then `PATH`.
    pub fn locate_compiler(explicit: Option<&Path>) -> PathBuf {
        if let Some(p) = explicit {
            return p.to_owned();
        }
        let name = if cfg!(windows) { "compile.exe" } else { "compile" };
        if let Ok(exe) = std::env::current_exe() {
            if let Some(sibling) = exe.parent().map(|d| d.join(name)) {
                if sibling.is_file() {
                    return sibling;
                }
            }
        }
        PathBuf::from(name)
    }

    fn run_compiler(&self, source: &str) -> Result<Report, String> {
        // The transpiler writes .smo/.h/.cpp files next to its inputs (including
        // the library), so every check works in a throw-away directory.
        let dir = tempfile::tempdir().map_err(|e| format!("cannot create temp dir: {e}"))?;
        let libs = dir.path().join("libs");
        copy_dir(&self.stdlib, &libs)
            .map_err(|e| format!("cannot read stdlib {}: {e}", self.stdlib.display()))?;
        std::fs::write(dir.path().join("strategy.sm"), source)
            .map_err(|e| format!("cannot write strategy: {e}"))?;

        let mut child = Command::new(&self.compiler)
            .current_dir(dir.path())
            .args(["-I", "libs", "strategy.sm"])
            .env("RUST_BACKTRACE", "0")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("cannot run compiler {}: {e}", self.compiler.display()))?;

        let start = Instant::now();
        let status = loop {
            match child.try_wait().map_err(|e| e.to_string())? {
                Some(status) => break status,
                None if start.elapsed() > self.timeout => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Ok(Report::failed("the compiler timed out"));
                }
                None => std::thread::sleep(Duration::from_millis(10)),
            }
        };
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        if status.success() {
            return Ok(Report::ok());
        }
        let stderr = String::from_utf8_lossy(&out.stderr);
        Ok(Report { ok: false, diagnostics: parse_diagnostics(&stderr) })
    }
}

impl Validator for Checker {
    fn check(&self, source: &str) -> Report {
        let mut report = match self.run_compiler(source) {
            Ok(r) => r,
            Err(e) => return Report::failed(format!("internal error: {e}")),
        };
        // The compiler does not verify `goto` targets, so lint the files it accepts.
        // (After a syntax error the lint would only add misleading follow-on noise.)
        let lints = if report.ok { lint(source) } else { vec![] };
        if !lints.is_empty() {
            report.ok = false;
            report.diagnostics.extend(lints);
        }
        report
    }
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// Turns the compiler's stderr into one line per problem, dropping panic noise.
fn parse_diagnostics(stderr: &str) -> Vec<String> {
    let mut lines: Vec<String> = stderr
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .filter_map(|l| l.strip_prefix("error: ").map(str::to_owned).or_else(|| Some(l.to_owned())))
        .collect();
    lines.truncate(20);
    if lines.is_empty() {
        lines.push("the compiler rejected the strategy without a message".into());
    }
    lines
}

/// Removes `//` and `/* */` comments, keeping line structure.
fn strip_comments(src: &str) -> String {
    let block = Regex::new(r"(?s)/\*.*?\*/").unwrap();
    let no_block = block.replace_all(src, |c: &regex::Captures| {
        "\n".repeat(c[0].matches('\n').count())
    });
    let line = Regex::new(r"//[^\n]*").unwrap();
    line.replace_all(&no_block, "").into_owned()
}

/// Structural checks the compiler does not perform. Returns one message per problem.
pub fn lint(source: &str) -> Vec<String> {
    let src = strip_comments(source);
    let mut problems = vec![];

    if !Regex::new(r"\bmachine\s+[A-Za-z_]").unwrap().is_match(&src) {
        problems.push("lint: no `machine` is defined".to_owned());
    }

    let declared: BTreeSet<&str> = Regex::new(r"\bstate\s+([A-Za-z_]\w*)")
        .unwrap()
        .captures_iter(&src)
        .map(|c| c.get(1).unwrap().as_str())
        .collect();

    let goto = Regex::new(r"\bgoto\s+([A-Za-z_]\w*)").unwrap();
    let launch = Regex::new(r"\blaunch\s*\(\s*\w+\s*(?:\([^)]*\))?\s*,\s*([A-Za-z_]\w*)\s*\)").unwrap();
    for (kind, re) in [("goto", &goto), ("launch", &launch)] {
        let mut seen = BTreeSet::new();
        for cap in re.captures_iter(&src) {
            let target = cap.get(1).unwrap().as_str();
            if !declared.contains(target) && seen.insert(target) {
                problems.push(format!(
                    "lint: `{kind} {target}` refers to a state that is never declared (declared states: {})",
                    if declared.is_empty() {
                        "none".to_owned()
                    } else {
                        declared.iter().copied().collect::<Vec<_>>().join(", ")
                    }
                ));
            }
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lint_accepts_the_shipped_examples() {
        for src in [include_str!("../../../examples/threshold.sm"), include_str!("../../../tests/example.sm")] {
            assert_eq!(lint(src), Vec::<String>::new());
        }
    }

    #[test]
    fn lint_finds_undeclared_goto_target() {
        let src = "machine M { process p { state A: goto B on x > 1; } }";
        let problems = lint(src);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("goto B"), "{problems:?}");
    }

    #[test]
    fn lint_finds_undeclared_launch_state() {
        let src = "machine M { init { launch (p(sym), START) for sym in s; } process p(sym: Symbol) { state A; } }";
        assert!(lint(src)[0].contains("launch START"));
    }

    #[test]
    fn lint_ignores_commented_out_code() {
        let src = "machine M { process p { state A: goto A; // goto NOPE\n /* goto NOPE2 */ } }";
        assert!(lint(src).is_empty());
    }

    #[test]
    fn lint_requires_a_machine() {
        assert!(lint("use std::*;")[0].contains("no `machine`"));
    }

    #[test]
    fn diagnostics_drop_prefix_and_blank_lines() {
        let d = parse_diagnostics("error: strategy.sm:(1,2): syntax error: expected x\n\n");
        assert_eq!(d, vec!["strategy.sm:(1,2): syntax error: expected x"]);
    }

    #[cfg(unix)]
    #[test]
    fn checker_reports_compiler_failure_and_success() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let libs = dir.path().join("libs");
        std::fs::create_dir(&libs).unwrap();
        std::fs::write(libs.join("std.sm"), "").unwrap();

        // A fake compiler that fails when the source contains BAD.
        let fake = dir.path().join("compile");
        std::fs::write(
            &fake,
            "#!/bin/sh\nif grep -q BAD strategy.sm; then echo 'error: strategy.sm:(1,1): boom' >&2; exit 1; fi\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

        let checker = Checker::new(fake, libs);
        assert!(checker.check("machine M { process p { state A; } }").ok);
        let bad = checker.check("machine M { BAD }");
        assert!(!bad.ok);
        assert_eq!(bad.diagnostics, vec!["strategy.sm:(1,1): boom"]);
    }
}
