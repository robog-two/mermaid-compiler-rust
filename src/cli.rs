//! Command-line parsing compatible with mermaid-cli 12 (commander.js semantics).
//!
//! Pure: `parse(args) -> Result<Command, CliError>`. Error texts match commander's
//! (`error: unknown option '--foo'`, `error: option '...' argument missing`, ...).

use crate::output::Format;
use crate::output::pdf::Paper;
use crate::theme::ThemeName;
use std::fmt;

/// Where `-o` points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputArg {
    Stdout,
    File(String),
}

/// Parsed options, before any filesystem checks.
#[derive(Debug, Clone, PartialEq)]
pub struct CliOptions {
    pub theme: Option<ThemeName>,
    pub size: Option<u32>,
    pub input: Option<String>,
    pub output: Option<OutputArg>,
    pub artefacts: Option<String>,
    pub jobs: Option<usize>,
    pub output_format: Option<Format>,
    pub background: String,
    pub config_file: Option<String>,
    pub css_file: Option<String>,
    pub svg_id: Option<String>,
    pub scale: f64,
    pub font_embed: bool,
    pub pdf_paper_format: Option<Paper>,
    pub quiet: bool,
    pub puppeteer_config_file: Option<String>,
    pub icon_packs: Vec<String>,
    pub icon_packs_names_and_urls: Vec<String>,
    /// Flags from older mmdc releases that are accepted but have no effect.
    pub ignored_legacy: Vec<String>,
}

impl Default for CliOptions {
    fn default() -> Self {
        CliOptions {
            theme: None,
            size: None,
            input: None,
            output: None,
            artefacts: None,
            jobs: None,
            output_format: None,
            background: "white".into(),
            config_file: None,
            css_file: None,
            svg_id: None,
            scale: 1.0,
            font_embed: true,
            pdf_paper_format: None,
            quiet: false,
            puppeteer_config_file: None,
            icon_packs: Vec::new(),
            icon_packs_names_and_urls: Vec::new(),
            ignored_legacy: Vec::new(),
        }
    }
}

/// What the user asked for.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Help,
    Version,
    Run(Box<CliOptions>),
}

/// A commander-style usage error (printed as `error: ...`, exit code 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliError(pub String);

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "error: {}", self.0)
    }
}

impl std::error::Error for CliError {}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Opt {
    Version,
    Help,
    Theme,
    Size,
    Input,
    Output,
    Artefacts,
    Jobs,
    OutputFormat,
    Background,
    ConfigFile,
    CssFile,
    SvgId,
    Scale,
    NoFontEmbed,
    PdfPaperFormat,
    Quiet,
    PuppeteerConfig,
    IconPacks,
    IconPacksNamesAndUrls,
    LegacyWidth,
    LegacyHeight,
    LegacyPdfFit,
}

struct Spec {
    opt: Opt,
    short: Option<char>,
    long: &'static str,
    /// Commander's flag description, used in error messages.
    flags: &'static str,
    takes_value: bool,
    variadic: bool,
}

const SPECS: &[Spec] = &[
    Spec {
        opt: Opt::Version,
        short: Some('V'),
        long: "version",
        flags: "-V, --version",
        takes_value: false,
        variadic: false,
    },
    Spec {
        opt: Opt::Help,
        short: Some('h'),
        long: "help",
        flags: "-h, --help",
        takes_value: false,
        variadic: false,
    },
    Spec {
        opt: Opt::Theme,
        short: Some('t'),
        long: "theme",
        flags: "-t, --theme <theme>",
        takes_value: true,
        variadic: false,
    },
    Spec {
        opt: Opt::Size,
        short: None,
        long: "size",
        flags: "--size <size>",
        takes_value: true,
        variadic: false,
    },
    Spec {
        opt: Opt::Input,
        short: Some('i'),
        long: "input",
        flags: "-i, --input <input>",
        takes_value: true,
        variadic: false,
    },
    Spec {
        opt: Opt::Output,
        short: Some('o'),
        long: "output",
        flags: "-o, --output <output>",
        takes_value: true,
        variadic: false,
    },
    Spec {
        opt: Opt::Artefacts,
        short: Some('a'),
        long: "artefacts",
        flags: "-a, --artefacts <artefacts>",
        takes_value: true,
        variadic: false,
    },
    Spec {
        opt: Opt::Jobs,
        short: Some('j'),
        long: "jobs",
        flags: "-j, --jobs <jobs>",
        takes_value: true,
        variadic: false,
    },
    Spec {
        opt: Opt::OutputFormat,
        short: Some('e'),
        long: "outputFormat",
        flags: "-e, --outputFormat <format>",
        takes_value: true,
        variadic: false,
    },
    Spec {
        opt: Opt::Background,
        short: Some('b'),
        long: "backgroundColor",
        flags: "-b, --backgroundColor <backgroundColor>",
        takes_value: true,
        variadic: false,
    },
    Spec {
        opt: Opt::ConfigFile,
        short: Some('c'),
        long: "configFile",
        flags: "-c, --configFile <configFile>",
        takes_value: true,
        variadic: false,
    },
    Spec {
        opt: Opt::CssFile,
        short: Some('C'),
        long: "cssFile",
        flags: "-C, --cssFile <cssFile>",
        takes_value: true,
        variadic: false,
    },
    Spec {
        opt: Opt::SvgId,
        short: Some('I'),
        long: "svgId",
        flags: "-I, --svgId <svgId>",
        takes_value: true,
        variadic: false,
    },
    Spec {
        opt: Opt::Scale,
        short: Some('s'),
        long: "scale",
        flags: "-s, --scale <scale>",
        takes_value: true,
        variadic: false,
    },
    Spec {
        opt: Opt::NoFontEmbed,
        short: None,
        long: "no-font-embed",
        flags: "--no-font-embed",
        takes_value: false,
        variadic: false,
    },
    Spec {
        opt: Opt::PdfPaperFormat,
        short: None,
        long: "pdf-paper-format",
        flags: "--pdf-paper-format <format>",
        takes_value: true,
        variadic: false,
    },
    Spec {
        opt: Opt::Quiet,
        short: Some('q'),
        long: "quiet",
        flags: "-q, --quiet",
        takes_value: false,
        variadic: false,
    },
    Spec {
        opt: Opt::PuppeteerConfig,
        short: Some('p'),
        long: "puppeteerConfigFile",
        flags: "-p --puppeteerConfigFile <puppeteerConfigFile>",
        takes_value: true,
        variadic: false,
    },
    Spec {
        opt: Opt::IconPacks,
        short: None,
        long: "iconPacks",
        flags: "--iconPacks <icon-packages...>",
        takes_value: true,
        variadic: true,
    },
    Spec {
        opt: Opt::IconPacksNamesAndUrls,
        short: None,
        long: "iconPacksNamesAndUrls",
        flags: "--iconPacksNamesAndUrls <prefix#iconsurl...>",
        takes_value: true,
        variadic: true,
    },
    // Removed upstream in mermaid-cli 10/11; accepted so old scripts keep working.
    Spec {
        opt: Opt::LegacyWidth,
        short: Some('w'),
        long: "width",
        flags: "-w, --width <width>",
        takes_value: true,
        variadic: false,
    },
    Spec {
        opt: Opt::LegacyHeight,
        short: Some('H'),
        long: "height",
        flags: "-H, --height <height>",
        takes_value: true,
        variadic: false,
    },
    Spec {
        opt: Opt::LegacyPdfFit,
        short: Some('f'),
        long: "pdfFit",
        flags: "-f, --pdfFit",
        takes_value: false,
        variadic: false,
    },
];

/// JavaScript `parseInt(value, 10)` semantics: leading digits, optional sign.
fn js_parse_int(s: &str) -> Option<i64> {
    let t = s.trim_start();
    let (sign, digits) = match t.strip_prefix('-') {
        Some(r) => (-1, r),
        None => (1, t.strip_prefix('+').unwrap_or(t)),
    };
    let d: String = digits.chars().take_while(|c| c.is_ascii_digit()).collect();
    d.parse::<i64>().ok().map(|v| v * sign)
}

/// JavaScript `parseFloat` semantics (longest valid numeric prefix).
fn js_parse_float(s: &str) -> Option<f64> {
    let t = s.trim_start();
    (1..=t.len())
        .rev()
        .filter(|&i| t.is_char_boundary(i))
        .find_map(|i| t[..i].parse::<f64>().ok().filter(|v| v.is_finite()))
}

fn invalid(spec: &Spec, value: &str, why: &str) -> CliError {
    CliError(format!(
        "option '{}' argument '{value}' is invalid. {why}",
        spec.flags
    ))
}

fn apply(
    o: &mut CliOptions,
    spec: &Spec,
    value: Option<String>,
) -> Result<Option<Command>, CliError> {
    let v = value.unwrap_or_default();
    match spec.opt {
        Opt::Version => return Ok(Some(Command::Version)),
        Opt::Help => return Ok(Some(Command::Help)),
        Opt::Theme => {
            let names: Vec<&str> = ThemeName::ALL.iter().map(|(n, _)| *n).collect();
            o.theme = Some(v.parse().map_err(|_| {
                invalid(
                    spec,
                    &v,
                    &format!("Allowed choices are {}.", names.join(", ")),
                )
            })?);
        }
        Opt::Size => {
            o.size = Some(
                js_parse_int(&v)
                    .filter(|n| *n >= 1)
                    .ok_or_else(|| invalid(spec, &v, "Not a positive integer."))?
                    as u32,
            )
        }
        Opt::Jobs => {
            o.jobs = Some(
                js_parse_int(&v)
                    .filter(|n| *n >= 1)
                    .ok_or_else(|| invalid(spec, &v, "Not a positive integer."))?
                    as usize,
            )
        }
        Opt::Scale => {
            o.scale = js_parse_float(&v)
                .filter(|n| *n > 0.0)
                .ok_or_else(|| invalid(spec, &v, "Not a positive number."))?
        }
        Opt::Input => o.input = Some(v),
        Opt::Output => {
            o.output = Some(if v == "-" {
                OutputArg::Stdout
            } else if [".svg", ".png", ".pdf", ".md", ".markdown"]
                .iter()
                .any(|e| v.ends_with(e))
            {
                OutputArg::File(v)
            } else {
                return Err(invalid(
                    spec,
                    &v,
                    "Output file must be either md, svg, png, or pdf.",
                ));
            })
        }
        Opt::Artefacts => o.artefacts = Some(v),
        Opt::OutputFormat => {
            o.output_format = Some(
                Format::from_ext(&v)
                    .ok_or_else(|| invalid(spec, &v, "Allowed choices are svg, png, pdf."))?,
            );
        }
        Opt::Background => o.background = v,
        Opt::ConfigFile => o.config_file = Some(v),
        Opt::CssFile => o.css_file = Some(v),
        Opt::SvgId => o.svg_id = Some(v),
        Opt::NoFontEmbed => o.font_embed = false,
        Opt::PdfPaperFormat => {
            o.pdf_paper_format = Some(v.parse().map_err(|_| {
                invalid(
                    spec,
                    &v,
                    &format!("Allowed choices are {}.", Paper::NAMES.join(", ")),
                )
            })?);
        }
        Opt::Quiet => o.quiet = true,
        Opt::PuppeteerConfig => o.puppeteer_config_file = Some(v),
        Opt::IconPacks => o.icon_packs.push(v),
        Opt::IconPacksNamesAndUrls => {
            if !v.contains('#') {
                return Err(invalid(
                    spec,
                    &v,
                    "Failed to parse a valid URL. The format should be iconPackName#https://my-example-url.example",
                ));
            }
            o.icon_packs_names_and_urls.push(v)
        }
        Opt::LegacyWidth | Opt::LegacyHeight | Opt::LegacyPdfFit => {
            o.ignored_legacy.push(spec.flags.to_string())
        }
    }
    Ok(None)
}

/// Parse `argv[1..]`.
pub fn parse(args: &[String]) -> Result<Command, CliError> {
    let mut o = CliOptions::default();
    let mut i = 0;
    let mut positional = Vec::new();
    while i < args.len() {
        let arg = &args[i];
        i += 1;
        if arg == "--" {
            positional.extend(args[i..].iter().cloned());
            break;
        }
        if let Some(long) = arg.strip_prefix("--") {
            let (name, inline) = match long.split_once('=') {
                Some((n, v)) => (n, Some(v.to_string())),
                None => (long, None),
            };
            let spec = SPECS
                .iter()
                .find(|s| s.long == name)
                .ok_or_else(|| CliError(format!("unknown option '--{name}'")))?;
            let value = if spec.takes_value {
                match inline {
                    Some(v) => Some(v),
                    None => {
                        let v = args.get(i).cloned().ok_or_else(|| {
                            CliError(format!("option '{}' argument missing", spec.flags))
                        })?;
                        i += 1;
                        Some(v)
                    }
                }
            } else {
                if inline.is_some() {
                    return Err(CliError(format!(
                        "option '{}' does not take an argument",
                        spec.flags
                    )));
                }
                None
            };
            if let Some(cmd) = apply(&mut o, spec, value)? {
                return Ok(cmd);
            }
            if spec.variadic {
                while let Some(next) = args.get(i).filter(|a| !a.starts_with('-')) {
                    apply(&mut o, spec, Some(next.clone()))?;
                    i += 1;
                }
            }
            continue;
        }
        if arg.len() > 1 && arg.starts_with('-') {
            let chars: Vec<char> = arg[1..].chars().collect();
            let mut k = 0;
            while k < chars.len() {
                let c = chars[k];
                k += 1;
                let spec = SPECS
                    .iter()
                    .find(|s| s.short == Some(c))
                    .ok_or_else(|| CliError(format!("unknown option '-{c}'")))?;
                let value = if spec.takes_value {
                    if k < chars.len() {
                        let v: String = chars[k..].iter().collect();
                        k = chars.len();
                        Some(v)
                    } else {
                        let v = args.get(i).cloned().ok_or_else(|| {
                            CliError(format!("option '{}' argument missing", spec.flags))
                        })?;
                        i += 1;
                        Some(v)
                    }
                } else {
                    None
                };
                if let Some(cmd) = apply(&mut o, spec, value)? {
                    return Ok(cmd);
                }
            }
            continue;
        }
        positional.push(arg.clone());
    }
    if !positional.is_empty() {
        return Err(CliError(format!(
            "too many arguments. Expected 0 arguments but got {}.",
            positional.len()
        )));
    }
    Ok(Command::Run(Box::new(o)))
}

/// `mmdc -h` text, laid out like commander's help.
pub fn help_text() -> String {
    let themes = ThemeName::ALL
        .iter()
        .map(|(n, _)| format!("\"{n}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let papers = Paper::NAMES
        .iter()
        .map(|n| format!("\"{n}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        r#"Usage: mmdc [options]

Render a mermaid diagram definition into various formats

Options:
  -V, --version                                   output the version number
  -t, --theme <theme>                             Theme of the chart (choices: {themes}, default: Mermaid default (depends on diagram type))
  --size <size>                                   Size of the diagram.
                                                  This will attempt to create a PNG with a maximum height or width equal to the given size.
                                                  For `.svg`s, this will set the `max-width` attribute.
                                                  For `.pdf`s, this is in CSS px, which is 1/96 of an inch.
  -i, --input <input>                             Input mermaid file. Files ending in .md will be treated as Markdown and all charts (e.g. ```mermaid (...)``` or :::mermaid (...):::) will be extracted and generated. Use `-` to read from stdin.
  -o, --output <output>                           Output file. It should be either md, svg, png, pdf or use `-` to output to stdout. Optional. Default: input + ".svg"
  -a, --artefacts <artefacts>                     Output artefacts path. Only used with Markdown input file. Optional. Default: output directory
  -j, --jobs <jobs>                               Number of parallel jobs to run when rendering multiple diagrams. Defaults to half the available CPUs.
  -e, --outputFormat <format>                     Output format for the generated image. (choices: "svg", "png", "pdf", default: Loaded from the output file extension)
  -b, --backgroundColor <backgroundColor>         Background color for pngs/svgs (not pdfs). Example: transparent, red, '#F0F0F0'. (default: "white")
  -c, --configFile <configFile>                   JSON configuration file for mermaid.
  -C, --cssFile <cssFile>                         CSS file for the page, used as Mermaid's `themeCSS`
  -I, --svgId <svgId>                             The id attribute for the SVG element to be rendered.
  -s, --scale <scale>                             Puppeteer scale factor (default: 1)
  --no-font-embed                                 Disable embedding fonts into the SVG. This cuts down on SVG size, but would result in missing fonts if they are not available on the system.
  --pdf-paper-format <format>                     If set, output a PDF with the given paper size, instead of fitting the diagram. (choices: {papers})
  -q, --quiet                                     Suppress log output
  -p --puppeteerConfigFile <puppeteerConfigFile>  JSON configuration file for puppeteer. (accepted for compatibility; no browser is used)
  --iconPacks <icon-packages...>                  Installed icon packs to use, e.g. @iconify-json/logos. (accepted for compatibility; icons are not rendered)
  --iconPacksNamesAndUrls <prefix#iconsurl...>    Icon packs to use, e.g. azure#file:///tmp/icons.json. (accepted for compatibility; icons are not rendered)
  -h, --help                                      display help for command
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    fn run(s: &[&str]) -> CliOptions {
        match parse(&args(s)).unwrap() {
            Command::Run(o) => *o,
            c => panic!("unexpected {c:?}"),
        }
    }

    #[test]
    fn basic_flags() {
        let o = run(&[
            "-i",
            "in.mmd",
            "-o",
            "out.png",
            "-t",
            "dark",
            "-b",
            "transparent",
            "-s",
            "2",
            "--size",
            "4000",
        ]);
        assert_eq!(o.input.as_deref(), Some("in.mmd"));
        assert_eq!(o.output, Some(OutputArg::File("out.png".into())));
        assert_eq!(o.theme, Some(ThemeName::Dark));
        assert_eq!(o.background, "transparent");
        assert_eq!(o.scale, 2.0);
        assert_eq!(o.size, Some(4000));
    }

    #[test]
    fn long_equals_and_clusters() {
        let o = run(&[
            "--input=a.mmd",
            "-qofoo.svg",
            "--no-font-embed",
            "-e",
            "pdf",
        ]);
        assert_eq!(o.input.as_deref(), Some("a.mmd"));
        assert!(o.quiet);
        assert_eq!(o.output, Some(OutputArg::File("foo.svg".into())));
        assert!(!o.font_embed);
        assert_eq!(o.output_format, Some(Format::Pdf));
    }

    #[test]
    fn stdout_and_variadic() {
        let o = run(&[
            "-o",
            "-",
            "--iconPacks",
            "@iconify-json/logos",
            "@iconify-json/mdi",
            "-q",
        ]);
        assert_eq!(o.output, Some(OutputArg::Stdout));
        assert_eq!(o.icon_packs.len(), 2);
        assert!(o.quiet);
    }

    #[test]
    fn commander_errors() {
        assert_eq!(
            parse(&args(&["--nope"])).unwrap_err().to_string(),
            "error: unknown option '--nope'"
        );
        assert_eq!(
            parse(&args(&["-i"])).unwrap_err().to_string(),
            "error: option '-i, --input <input>' argument missing"
        );
        assert!(
            parse(&args(&["-o", "x.txt"]))
                .unwrap_err()
                .0
                .contains("Output file must be either md, svg, png, or pdf.")
        );
        assert!(
            parse(&args(&["-t", "sparkly"]))
                .unwrap_err()
                .0
                .contains("Allowed choices are base, dark")
        );
        assert!(
            parse(&args(&["--size", "0"]))
                .unwrap_err()
                .0
                .contains("Not a positive integer.")
        );
        assert!(
            parse(&args(&["-s", "abc"]))
                .unwrap_err()
                .0
                .contains("Not a positive number.")
        );
        assert!(
            parse(&args(&["stray"]))
                .unwrap_err()
                .0
                .starts_with("too many arguments")
        );
    }

    #[test]
    fn help_and_version_short_circuit() {
        assert_eq!(parse(&args(&["-h", "--nope"])).unwrap(), Command::Help);
        assert_eq!(parse(&args(&["--version"])).unwrap(), Command::Version);
    }

    #[test]
    fn js_number_parsing() {
        assert_eq!(js_parse_int("12abc"), Some(12));
        assert_eq!(js_parse_int("abc"), None);
        assert_eq!(js_parse_float("1.5x"), Some(1.5));
    }

    #[test]
    fn legacy_flags_are_tolerated() {
        let o = run(&["-w", "1024", "-H", "768", "-f"]);
        assert_eq!(o.ignored_legacy.len(), 3);
    }
}
