//! End-to-end tests of the `mmdc` binary's observable behaviour (files, stdout,
//! stderr, exit codes), mirroring mermaid-cli 12.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Fresh scratch directory per test (no tempfile crate needed).
fn scratch() -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("mmdc-test-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn mmdc(dir: &Path, args: &[&str], stdin: Option<&str>) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_mmdc"))
        .args(args)
        .current_dir(dir)
        .env("NO_COLOR", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    {
        let mut si = child.stdin.take().unwrap();
        if let Some(s) = stdin {
            si.write_all(s.as_bytes()).unwrap();
        }
    }
    child.wait_with_output().unwrap()
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

const FLOW: &str = "graph TD\n  A[Start] --> B{Ok?}\n  B -->|yes| C[Done]\n";

#[test]
fn default_output_is_input_plus_svg() {
    let d = scratch();
    std::fs::write(d.join("in.mmd"), FLOW).unwrap();
    let out = mmdc(&d, &["-i", "in.mmd"], None);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(text(&out.stdout).contains("Generating single mermaid chart"));
    let svg = std::fs::read_to_string(d.join("in.mmd.svg")).unwrap();
    assert!(svg.starts_with("<svg id=\"my-svg\" width=\"100%\""));
    assert!(svg.contains("aria-roledescription=\"flowchart-v2\""));
    assert!(svg.contains("style=\"max-width: "));
    assert!(svg.contains("background-color: white;"));
}

#[test]
fn stdin_to_stdout() {
    let d = scratch();
    let out = mmdc(&d, &["-i", "-", "-o", "-"], Some(FLOW));
    assert!(out.status.success());
    assert!(text(&out.stdout).starts_with("<svg"));
    assert!(text(&out.stderr).contains("No output format specified, using svg."));
    // stdout output forces quiet: nothing but the SVG on stdout.
    assert!(!text(&out.stdout).contains("Generating"));
}

#[test]
fn missing_input_flag_warns_and_reads_stdin() {
    let d = scratch();
    let out = mmdc(&d, &["-e", "svg"], Some(FLOW));
    assert!(out.status.success());
    assert!(text(&out.stderr).contains("No input file specified, reading from stdin."));
    assert!(d.join("out.svg").exists());
}

#[test]
fn quiet_suppresses_info() {
    let d = scratch();
    std::fs::write(d.join("in.mmd"), FLOW).unwrap();
    let out = mmdc(&d, &["-q", "-i", "in.mmd", "-o", "o.svg"], None);
    assert!(out.status.success());
    assert!(out.stdout.is_empty());
}

#[test]
fn error_paths_exit_1_with_mmdc_messages() {
    let d = scratch();
    std::fs::write(d.join("in.mmd"), FLOW).unwrap();
    let cases: &[(&[&str], &str)] = &[
        (&["-i", "nope.mmd"], "Input file \"nope.mmd\" doesn't exist"),
        (
            &["-i", "in.mmd", "-o", "missing/dir/o.svg"],
            "Output directory \"missing/dir/\" doesn't exist",
        ),
        (
            &["-i", "in.mmd", "-o", "o.txt"],
            "Output file must be either md, svg, png, or pdf.",
        ),
        (
            &["-i", "in.mmd", "-c", "nope.json"],
            "Configuration file \"nope.json\" doesn't exist",
        ),
        (
            &["-i", "in.mmd", "-C", "nope.css"],
            "CSS file \"nope.css\" doesn't exist",
        ),
        (
            &["-i", "in.mmd", "-a", "art"],
            "Artefacts [-a|--artefacts] path can only be used with Markdown input file",
        ),
        (&["--bogus"], "error: unknown option '--bogus'"),
        (&["-t", "pink"], "Allowed choices are base, dark, default"),
    ];
    for (args, msg) in cases {
        let out = mmdc(&d, args, Some(""));
        assert_eq!(out.status.code(), Some(1), "args {args:?}");
        assert!(
            text(&out.stderr).contains(msg),
            "args {args:?}: stderr was {}",
            text(&out.stderr)
        );
    }
}

#[test]
fn parse_errors_report_line_numbers() {
    let d = scratch();
    std::fs::write(d.join("bad.mmd"), "graph TD\nA --> B\nA -->\n").unwrap();
    let out = mmdc(&d, &["-i", "bad.mmd", "-o", "bad.svg"], None);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("Parse error on line 3"));
    assert!(!d.join("bad.svg").exists());

    std::fs::write(d.join("x.mmd"), "this is not mermaid").unwrap();
    let out = mmdc(&d, &["-i", "x.mmd"], None);
    assert!(text(&out.stderr).contains("No diagram type detected matching given configuration"));
}

#[test]
fn markdown_mode_rewrites_document() {
    let d = scratch();
    let md = "# Title\n\n```mermaid\ngraph TD\nA-->B\n```\n\ntext\n\n```mermaid\nsequenceDiagram\naccTitle: My title\naccDescr: My description\nA->>B: hi\n```\n\n```js\nnot mermaid\n```\n";
    std::fs::write(d.join("readme.template.md"), md).unwrap();
    let out = mmdc(&d, &["-i", "readme.template.md", "-o", "readme.md"], None);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let log = text(&out.stdout);
    assert!(log.contains("Found 2 mermaid charts in Markdown input"));
    assert!(log.contains(" ✅ ./readme-1.svg"));
    assert!(log.contains(" ✅ readme.md"));
    let result = std::fs::read_to_string(d.join("readme.md")).unwrap();
    assert!(result.contains("![diagram](./readme-1.svg)"));
    assert!(result.contains("![My description](./readme-2.svg \"My title\")"));
    assert!(result.contains("```js\nnot mermaid\n```"));
    assert!(d.join("readme-1.svg").exists() && d.join("readme-2.svg").exists());
    let svg2 = std::fs::read_to_string(d.join("readme-2.svg")).unwrap();
    assert!(svg2.contains("<title id=\"chart-title-my-svg\">My title</title>"));
}

#[test]
fn markdown_with_artefacts_and_png_template() {
    let d = scratch();
    std::fs::create_dir_all(d.join("docs")).unwrap();
    std::fs::write(d.join("docs/in.md"), "```mermaid\npie\n\"a\": 1\n```\n").unwrap();
    let out = mmdc(
        &d,
        &["-i", "docs/in.md", "-o", "docs/out.md", "-a", "img/gen"],
        None,
    );
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(d.join("img/gen/out-1.svg").exists());
    let result = std::fs::read_to_string(d.join("docs/out.md")).unwrap();
    assert_eq!(result, "![diagram](./../img/gen/out-1.svg)\n");

    let out = mmdc(&d, &["-i", "docs/in.md", "-o", "docs/chart.pdf"], None);
    assert!(out.status.success());
    assert!(d.join("docs/chart-1.pdf").exists());

    let out = mmdc(&d, &["-i", "docs/in.md", "-o", "-"], None);
    assert!(text(&out.stderr).contains("Cannot use `stdout` with markdown input"));
}

#[test]
fn markdown_without_charts() {
    let d = scratch();
    std::fs::write(d.join("a.md"), "just text\n").unwrap();
    let out = mmdc(&d, &["-i", "a.md", "-o", "b.md"], None);
    assert!(out.status.success());
    assert!(text(&out.stdout).contains("No mermaid charts found in Markdown input"));
    assert_eq!(
        std::fs::read_to_string(d.join("b.md")).unwrap(),
        "just text\n"
    );
}

#[test]
fn svg_options_id_background_size_css_theme() {
    let d = scratch();
    std::fs::write(d.join("in.mmd"), FLOW).unwrap();
    std::fs::write(d.join("style.css"), ".node rect { fill: hotpink; }").unwrap();
    std::fs::write(d.join("cfg.json"), r#"{"theme":"dark","themeCSS":".x{}"}"#).unwrap();
    let out = mmdc(
        &d,
        &[
            "-i",
            "in.mmd",
            "-o",
            "o.svg",
            "-I",
            "custom-id",
            "-b",
            "transparent",
            "--size",
            "1200",
            "-C",
            "style.css",
            "-c",
            "cfg.json",
        ],
        None,
    );
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(
        text(&out.stderr)
            .contains("Overriding existing themeCSS in mermaidConfig with CSS from \"style.css\"")
    );
    let svg = std::fs::read_to_string(d.join("o.svg")).unwrap();
    assert!(svg.starts_with("<svg id=\"custom-id\""));
    assert!(svg.contains("background-color: transparent;"));
    assert!(svg.contains("max-width: 1200px;") && svg.contains("max-height: 1200px;"));
    assert!(svg.contains(".node rect { fill: hotpink; }"));
    assert!(svg.contains("#1f2020"), "dark theme node fill");
}

#[test]
fn pdf_output() {
    let d = scratch();
    std::fs::write(d.join("in.mmd"), FLOW).unwrap();
    let out = mmdc(&d, &["-i", "in.mmd", "-o", "o.pdf"], None);
    assert!(out.status.success());
    let pdf = std::fs::read(d.join("o.pdf")).unwrap();
    assert!(pdf.starts_with(b"%PDF-1.4"));
    assert!(pdf.ends_with(b"%%EOF\n"));
    let out = mmdc(
        &d,
        &["-i", "in.mmd", "-o", "a4.pdf", "--pdf-paper-format", "A4"],
        None,
    );
    assert!(out.status.success());
    assert!(
        text(&std::fs::read(d.join("a4.pdf")).unwrap()).contains("/MediaBox [0 0 595.44 842.4]")
    );
}

#[cfg(feature = "png")]
#[test]
fn png_output_respects_scale_and_size() {
    fn dims(png: &[u8]) -> (u32, u32) {
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        let w = u32::from_be_bytes(png[16..20].try_into().unwrap());
        let h = u32::from_be_bytes(png[20..24].try_into().unwrap());
        (w, h)
    }
    let d = scratch();
    std::fs::write(d.join("in.mmd"), FLOW).unwrap();
    assert!(
        mmdc(&d, &["-i", "in.mmd", "-o", "a.png"], None)
            .status
            .success()
    );
    assert!(
        mmdc(&d, &["-i", "in.mmd", "-o", "b.png", "-s", "2"], None)
            .status
            .success()
    );
    assert!(
        mmdc(&d, &["-i", "in.mmd", "-o", "c.png", "--size", "1000"], None)
            .status
            .success()
    );
    let (w1, h1) = dims(&std::fs::read(d.join("a.png")).unwrap());
    let (w2, h2) = dims(&std::fs::read(d.join("b.png")).unwrap());
    let (w3, h3) = dims(&std::fs::read(d.join("c.png")).unwrap());
    assert!((w2 as i64 - 2 * w1 as i64).abs() <= 1 && (h2 as i64 - 2 * h1 as i64).abs() <= 1);
    assert_eq!(h3.max(w3), 1000, "larger side equals --size");
}

#[test]
fn version_and_help() {
    let d = scratch();
    let out = mmdc(&d, &["-V"], None);
    assert!(out.status.success());
    assert_eq!(text(&out.stdout).trim(), env!("CARGO_PKG_VERSION"));
    let out = mmdc(&d, &["--help"], None);
    assert!(text(&out.stdout).starts_with("Usage: mmdc [options]"));
}

#[test]
fn every_example_renders_in_every_format() {
    let d = scratch();
    let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/diagrams");
    let mut count = 0;
    for entry in std::fs::read_dir(examples).unwrap() {
        let p = entry.unwrap().path();
        if p.extension().and_then(|e| e.to_str()) != Some("mmd") {
            continue;
        }
        count += 1;
        let formats: &[&str] = if cfg!(feature = "png") {
            &["svg", "pdf", "png"]
        } else {
            &["svg", "pdf"]
        };
        for fmt in formats {
            let out_file = d.join(format!(
                "{}.{fmt}",
                p.file_stem().unwrap().to_string_lossy()
            ));
            let out = mmdc(
                &d,
                &[
                    "-q",
                    "-i",
                    p.to_str().unwrap(),
                    "-o",
                    out_file.to_str().unwrap(),
                ],
                None,
            );
            assert!(
                out.status.success(),
                "{} → {fmt}: {}",
                p.display(),
                text(&out.stderr)
            );
            assert!(std::fs::metadata(&out_file).unwrap().len() > 200);
        }
    }
    assert!(count >= 5);
}
