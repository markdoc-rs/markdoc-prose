//! `markdoc-prose` — the command line face of the prose rules.
//!
//! Usage:
//!     markdoc-prose [--json] [--root <dir>] [path…]
//!
//! Exit code is 1 if anything was reported at `error`, 0 otherwise. A
//! warning does not fail a build: a writer who cannot merge until every
//! style hint is gone stops reading them.

use markdoc_lint::{Severity, corpus};
use markdoc_prose::{CONFIG_FILENAME, Config, Prose};
use std::path::{Path, PathBuf};

fn main() -> std::process::ExitCode {
    let mut root = PathBuf::from(".");
    let mut json = false;
    let mut list_rules = false;
    let mut paths: Vec<PathBuf> = Vec::new();
    let mut args = std::env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--json" => json = true,
            "--rules" => list_rules = true,
            "--root" => match args.next() {
                Some(v) => root = PathBuf::from(v),
                None => return fail("--root needs a directory"),
            },
            "-h" | "--help" => {
                println!("{HELP}");
                return std::process::ExitCode::SUCCESS;
            }
            "--version" => {
                println!("markdoc-prose {}", env!("CARGO_PKG_VERSION"));
                return std::process::ExitCode::SUCCESS;
            }
            other if other.starts_with('-') => return fail(&format!("unknown option {other}")),
            other => paths.push(PathBuf::from(other)),
        }
    }

    let root = match root.canonicalize() {
        Ok(r) => r,
        Err(e) => return fail(&format!("{}: {e}", root.display())),
    };

    // A malformed config is fatal rather than ignored. Falling back to the
    // defaults would turn a typo into "the linter stopped finding things".
    let config = match Config::load(&root) {
        Ok(c) => c,
        Err(e) => return fail(&e),
    };
    let configured = root.join(CONFIG_FILENAME).is_file();
    let mut prose = match Prose::new(&config, &root) {
        Ok(p) => p,
        Err(e) => return fail(&e),
    };

    // A path argument is resolved against the working directory, as a shell
    // does, and then has to end up under the root -- otherwise it is linted
    // with a relative path that no rule's skip list recognises and reported
    // against a file name nobody can find.
    if list_rules {
        for (name, enabled) in prose.rules() {
            println!("{} {name}", if enabled { "on " } else { "off" });
        }
        return std::process::ExitCode::SUCCESS;
    }

    let targets: Vec<PathBuf> = if paths.is_empty() {
        corpus::find_files(&root, &|p| {
            p.extension().and_then(|s| s.to_str()) == Some("md")
        })
    } else {
        paths
            .iter()
            .filter_map(|p| {
                p.canonicalize()
                    .or_else(|_| root.join(p).canonicalize())
                    .ok()
            })
            .filter(|p| {
                if p.starts_with(&root) {
                    true
                } else {
                    eprintln!(
                        "markdoc-prose: {} is outside {}, skipping",
                        p.display(),
                        root.display()
                    );
                    false
                }
            })
            .flat_map(|p| {
                if p.is_dir() {
                    corpus::find_files(&p, &|q| {
                        q.extension().and_then(|s| s.to_str()) == Some("md")
                    })
                } else {
                    vec![p]
                }
            })
            .collect()
    };

    // The skip list is markdoc-lint's, read from the same place: a file the
    // structural rules do not apply to is not a file the prose rules apply
    // to either, and two lists would drift.
    let lint_config = markdoc_lint::Config::load(&root).unwrap_or_default();

    let mut all = Vec::new();
    let mut linted = 0usize;
    for file in &targets {
        let Some(rel) = rel_of(&root, file) else {
            continue;
        };
        if lint_config.is_skipped(&rel) {
            continue;
        }
        let Ok(source) = std::fs::read_to_string(file) else {
            continue;
        };
        linted += 1;
        all.extend(prose.lint(&rel, &source));
    }

    let errors = all.iter().filter(|d| d.severity == Severity::Error).count();
    let warnings = all
        .iter()
        .filter(|d| d.severity == Severity::Warning)
        .count();

    if json {
        let out = serde_json::json!({
            "files": linted,
            "configured": configured,
            "errors": errors,
            "warnings": warnings,
            "diagnostics": all,
        });
        println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
    } else {
        for d in &all {
            println!("{}", d.to_text());
        }
        eprintln!("\n{linted} file(s) checked; {errors} error(s), {warnings} warning(s).");
        if !configured {
            eprintln!(
                "note: no {CONFIG_FILENAME} in {}, so only the engine's own rules ran.",
                root.display()
            );
        }
    }

    if errors > 0 {
        std::process::ExitCode::FAILURE
    } else {
        std::process::ExitCode::SUCCESS
    }
}

fn rel_of(root: &Path, file: &Path) -> Option<String> {
    Some(
        file.strip_prefix(root)
            .ok()?
            .to_string_lossy()
            .replace('\\', "/"),
    )
}

fn fail(msg: &str) -> std::process::ExitCode {
    eprintln!("markdoc-prose: {msg}\n\n{HELP}");
    std::process::ExitCode::FAILURE
}

const HELP: &str = "\
markdoc-prose [--json] [--root <dir>] [path…]

  --root <dir>   the content root. Defaults to the working directory.
  --json         one JSON object on stdout, for CI.
  --rules        list every rule the engine has and whether it is on.
  --version      print the version.

Rules come from markdoc-prose.toml at the root: corrections from the
weirpack it names, and patterns that cannot name a correction from its own
`[[rule]]` entries. Files are skipped according to markdoc-lint.toml.

Exits 1 if anything was reported at error level.";
