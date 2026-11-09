//! Imperative shell: argument handling, file/stdin/stdout I/O, logging, exit codes.
//! Every decision is delegated to the pure library (`mmdc::*`).

#![forbid(unsafe_code)]

use mmdc::cli::{self, CliOptions, Command, OutputArg};
use mmdc::config::MermaidConfig;
use mmdc::markdown::{self, MarkdownImage};
use mmdc::output::{self, EncodeOptions, Format};
use mmdc::{json, plan, render};
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[cfg(feature = "png")]
type Rasterizer = mmdc::output::png::Rasterizer;
#[cfg(not(feature = "png"))]
struct Rasterizer;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    match cli::parse(&args) {
        Ok(Command::Help) => {
            print!("{}", cli::help_text());
            ExitCode::SUCCESS
        }
        Ok(Command::Version) => {
            println!("{}", mmdc::VERSION);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(1)
        }
        Ok(Command::Run(opts)) => {
            let mut log = Log::new(opts.quiet);
            match run(*opts, &mut log) {
                Ok(()) => ExitCode::SUCCESS,
                Err(msg) => {
                    log.error(&msg);
                    ExitCode::from(1)
                }
            }
        }
    }
}

/// Console output with chalk-like colouring (only when stderr is a terminal).
struct Log {
    quiet: bool,
    color: bool,
}

impl Log {
    fn new(quiet: bool) -> Self {
        let color = std::env::var_os("NO_COLOR").is_none()
            && (std::env::var_os("FORCE_COLOR").is_some() || io::stderr().is_terminal());
        Log { quiet, color }
    }
    fn info(&self, msg: &str) {
        if !self.quiet {
            println!("{msg}");
        }
    }
    fn paint(&self, code: &str, msg: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{msg}\x1b[39m")
        } else {
            msg.to_string()
        }
    }
    fn warn(&self, msg: &str) {
        eprintln!("{}", self.paint("33", &format!("\n{msg}\n")));
    }
    fn error(&self, msg: &str) {
        eprintln!("{}", self.paint("31", &format!("\n{msg}\n")));
    }
}

fn read_json_file(path: &str, what: &str) -> Result<json::Json, String> {
    if !Path::new(path).exists() {
        return Err(format!("{what} \"{path}\" doesn't exist"));
    }
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("Failed to read \"{path}\": {e}"))?;
    json::parse(&text).map_err(|e| format!("SyntaxError: {e} in JSON file \"{path}\""))
}

fn run(opts: CliOptions, log: &mut Log) -> Result<(), String> {
    // Input.
    let input: Option<String> = match opts.input.as_deref() {
        None => {
            log.warn("No input file specified, reading from stdin. If you want to specify an input file, please use `-i <input>.` You can use `-i -` to read from stdin and to suppress this warning.");
            None
        }
        Some("-") => None,
        Some(p) => {
            if !Path::new(p).exists() {
                return Err(format!("Input file \"{p}\" doesn't exist"));
            }
            Some(p.to_string())
        }
    };

    // Output.
    let mut format = opts.output_format;
    let output = match opts.output.clone() {
        None => OutputArg::File(plan::default_output(input.as_deref(), format)),
        Some(OutputArg::Stdout) => {
            log.quiet = true;
            if format.is_none() {
                format = Some(Format::Svg);
                log.warn("No output format specified, using svg. If you want to specify an output format and suppress this warning, please use `-e <format>.` ");
            }
            OutputArg::Stdout
        }
        Some(f) => f,
    };

    if let Some(a) = &opts.artefacts {
        if !input.as_deref().is_some_and(plan::is_markdown) {
            return Err(
                "Artefacts [-a|--artefacts] path can only be used with Markdown input file".into(),
            );
        }
        std::fs::create_dir_all(a)
            .map_err(|e| format!("Failed to create artefacts directory \"{a}\": {e}"))?;
    }
    if let OutputArg::File(f) = &output {
        let dir = plan::output_dir(f);
        if !Path::new(&dir).exists() {
            return Err(format!("Output directory \"{dir}/\" doesn't exist"));
        }
    }

    // Configuration: -t, then --configFile (assigned over it), then --cssFile.
    let mut config = MermaidConfig::default();
    if let Some(t) = opts.theme {
        config.theme = t;
    }
    let mut has_theme_css = false;
    if let Some(path) = &opts.config_file {
        let j = read_json_file(path, "Configuration file")?;
        has_theme_css = j.get("themeCSS").is_some();
        config = config
            .with_json(&j)
            .map_err(|e| format!("Invalid configuration file \"{path}\": {e}"))?;
    }
    if let Some(path) = &opts.puppeteer_config_file {
        read_json_file(path, "Configuration file")?; // validated like mmdc, otherwise unused
    }
    if let Some(css) = &opts.css_file {
        if !Path::new(css).exists() {
            return Err(format!("CSS file \"{css}\" doesn't exist"));
        }
        if has_theme_css {
            log.warn(&format!(
                "Overriding existing themeCSS in mermaidConfig with CSS from \"{css}\""
            ));
        }
        config.theme_css = Some(
            std::fs::read_to_string(css).map_err(|e| format!("Failed to read \"{css}\": {e}"))?,
        );
    }
    if !opts.icon_packs.is_empty() || !opts.icon_packs_names_and_urls.is_empty() {
        log.warn("Icon packs are accepted for compatibility but are not rendered by this implementation.");
    }
    for flag in &opts.ignored_legacy {
        log.warn(&format!(
            "Option '{flag}' is no longer supported by mermaid-cli and is ignored."
        ));
    }

    let encode = EncodeOptions {
        svg_id: opts.svg_id.clone().unwrap_or_else(|| "my-svg".into()),
        background: opts.background.clone(),
        size: opts.size,
        scale: opts.scale,
        paper: opts.pdf_paper_format,
    };
    let jobs = opts.jobs.unwrap_or_else(|| {
        (std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(2)
            / 2)
        .max(1)
    });

    let output_name = match &output {
        OutputArg::Stdout => "/dev/stdout".to_string(),
        OutputArg::File(f) => f.clone(),
    };
    let format = plan::infer_format(&output_name, format)?;
    let rasterizer = make_rasterizer(format)?;

    let definition = match &input {
        Some(p) => {
            std::fs::read_to_string(p).map_err(|e| format!("Failed to read \"{p}\": {e}"))?
        }
        None => {
            let mut s = String::new();
            io::stdin()
                .read_to_string(&mut s)
                .map_err(|e| format!("Failed to read stdin: {e}"))?;
            s
        }
    };

    if input.as_deref().is_some_and(plan::is_markdown) {
        if matches!(output, OutputArg::Stdout) {
            return Err("Cannot use `stdout` with markdown input".into());
        }
        let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
        let charts = markdown::find_charts(&definition);
        let targets: Vec<(PathBuf, String)> = (1..=charts.len())
            .map(|n| {
                let file = plan::markdown_image_file(
                    &output_name,
                    n,
                    format,
                    opts.artefacts.as_deref().map(Path::new),
                    &cwd,
                );
                let url = plan::markdown_image_url(&output_name, &file, &cwd);
                (file, url)
            })
            .collect();
        let defs: Vec<&str> = charts.iter().map(|c| c.definition).collect();
        let results = render_parallel(&defs, &config, &encode, format, rasterizer.as_ref(), jobs);
        if charts.is_empty() {
            log.info("No mermaid charts found in Markdown input");
        } else {
            log.info(&format!(
                "Found {} mermaid charts in Markdown input",
                charts.len()
            ));
        }
        let mut images = Vec::with_capacity(results.len());
        for ((file, url), result) in targets.iter().zip(results) {
            let (bytes, title, desc) = result?;
            std::fs::write(file, bytes)
                .map_err(|e| format!("Failed to write \"{}\": {e}", file.display()))?;
            log.info(&format!(" ✅ {url}"));
            images.push(MarkdownImage {
                url: url.clone(),
                alt: desc.unwrap_or_else(|| "diagram".into()),
                title,
            });
        }
        if plan::is_markdown(&output_name) {
            let out = markdown::replace_charts(&definition, &charts, &images);
            std::fs::write(&output_name, out)
                .map_err(|e| format!("Failed to write \"{output_name}\": {e}"))?;
            log.info(&format!(" ✅ {output_name}"));
        }
        return Ok(());
    }

    log.info("Generating single mermaid chart");
    let (bytes, _, _) = render_one(&definition, &config, &encode, format, rasterizer.as_ref())?;
    match &output {
        OutputArg::Stdout => {
            let mut out = io::stdout().lock();
            out.write_all(&bytes)
                .and_then(|_| out.flush())
                .map_err(|e| format!("Failed to write to stdout: {e}"))?;
        }
        OutputArg::File(f) => {
            std::fs::write(f, bytes).map_err(|e| format!("Failed to write \"{f}\": {e}"))?
        }
    }
    Ok(())
}

#[cfg(feature = "png")]
fn make_rasterizer(format: Format) -> Result<Option<Rasterizer>, String> {
    Ok((format == Format::Png).then(Rasterizer::new))
}

#[cfg(not(feature = "png"))]
fn make_rasterizer(format: Format) -> Result<Option<Rasterizer>, String> {
    if format == Format::Png {
        return Err(
            "PNG output needs the `png` feature; rebuild without --no-default-features".into(),
        );
    }
    Ok(None)
}

type Rendered = (Vec<u8>, Option<String>, Option<String>);

fn render_one(
    def: &str,
    config: &MermaidConfig,
    enc: &EncodeOptions,
    format: Format,
    rast: Option<&Rasterizer>,
) -> Result<Rendered, String> {
    let r = render::render(def, config).map_err(|e| format!("Error: {e}"))?;
    let bytes = match format {
        Format::Svg => output::encode_svg(&r, enc).into_bytes(),
        Format::Pdf => output::encode_pdf(&r, enc),
        #[cfg(feature = "png")]
        Format::Png => {
            output::encode_png(&r, enc, rast.ok_or("internal error: rasterizer missing")?)?
        }
        #[cfg(not(feature = "png"))]
        Format::Png => {
            let _ = rast;
            return Err("PNG output is not available in this build".into());
        }
    };
    Ok((
        bytes,
        r.title().map(str::to_string),
        r.description().map(str::to_string),
    ))
}

/// Render many definitions on up to `jobs` threads, preserving order.
fn render_parallel(
    defs: &[&str],
    config: &MermaidConfig,
    enc: &EncodeOptions,
    format: Format,
    rast: Option<&Rasterizer>,
    jobs: usize,
) -> Vec<Result<Rendered, String>> {
    if jobs <= 1 || defs.len() <= 1 {
        return defs
            .iter()
            .map(|d| render_one(d, config, enc, format, rast))
            .collect();
    }
    let chunk = defs.len().div_ceil(jobs);
    std::thread::scope(|s| {
        let handles: Vec<_> = defs
            .chunks(chunk)
            .map(|part| {
                s.spawn(move || {
                    part.iter()
                        .map(|d| render_one(d, config, enc, format, rast))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| {
                h.join()
                    .unwrap_or_else(|_| vec![Err("render thread panicked".into())])
            })
            .collect()
    })
}
