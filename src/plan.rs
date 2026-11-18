//! Pure decisions the CLI makes about file names and formats, separated from I/O
//! so they can be unit-tested. Paths are handled lexically (no filesystem access).

use crate::output::Format;
use std::path::{Component, Path, PathBuf};

/// `/\.(md|markdown)$/`
pub fn is_markdown(path: &str) -> bool {
    path.ends_with(".md") || path.ends_with(".markdown")
}

/// mmdc's default output: `input + ".svg"` (or `.{format}`), `out.svg` for stdin.
pub fn default_output(input: Option<&str>, format: Option<Format>) -> String {
    let ext = format.map(Format::ext).unwrap_or("svg");
    match input {
        Some(i) => format!("{i}.{ext}"),
        None => format!("out.{ext}"),
    }
}

/// Format from `-e`, else from the output extension (Markdown output ⇒ svg).
pub fn infer_format(output: &str, explicit: Option<Format>) -> Result<Format, String> {
    if let Some(f) = explicit {
        return Ok(f);
    }
    let ext = Path::new(output)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    if ext == "md" || ext == "markdown" {
        return Ok(Format::Svg);
    }
    Format::from_ext(ext)
        .ok_or_else(|| "Output format must be one of \"svg\", \"png\" or \"pdf\"".to_string())
}

/// Image file for the `n`-th (1-based) chart of a Markdown input:
/// `out.md` → `out-1.svg`; `out.png` → `out-1.png`; with `artefacts`, placed there.
pub fn markdown_image_file(
    output: &str,
    n: usize,
    format: Format,
    artefacts: Option<&Path>,
    cwd: &Path,
) -> PathBuf {
    let mut file = output.to_string();
    for ext in [".markdown", ".md", ".png", ".svg", ".pdf"] {
        if let Some(stem) = output.strip_suffix(ext) {
            file = format!("{stem}-{n}{ext}");
            break;
        }
    }
    for ext in [".markdown", ".md"] {
        if let Some(stem) = file.strip_suffix(ext) {
            file = format!("{stem}.{}", format.ext());
            break;
        }
    }
    match artefacts {
        Some(dir) => {
            let base = Path::new(&file)
                .file_name()
                .map(PathBuf::from)
                .unwrap_or_default();
            absolutize(cwd, &dir.join(base))
        }
        None => PathBuf::from(file),
    }
}

/// `"./" + path.relative(dirname(resolve(output)), resolve(file))`
pub fn markdown_image_url(output: &str, file: &Path, cwd: &Path) -> String {
    let out_abs = absolutize(cwd, Path::new(output));
    let dir = out_abs
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("/"));
    let file_abs = absolutize(cwd, file);
    format!("./{}", relative(&dir, &file_abs))
}

/// Lexical `path.resolve(cwd, p)`.
pub fn absolutize(cwd: &Path, p: &Path) -> PathBuf {
    let joined = if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    };
    let mut out = PathBuf::new();
    for c in joined.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Lexical `path.relative(from, to)` for absolute, normalised paths.
pub fn relative(from: &Path, to: &Path) -> String {
    let f: Vec<_> = from.components().collect();
    let t: Vec<_> = to.components().collect();
    let common = f.iter().zip(&t).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<String> = std::iter::repeat_n("..".to_string(), f.len() - common).collect();
    parts.extend(
        t[common..]
            .iter()
            .map(|c| c.as_os_str().to_string_lossy().into_owned()),
    );
    parts.join("/")
}

/// `path.dirname` for the "output directory doesn't exist" check.
pub fn output_dir(output: &str) -> String {
    match Path::new(output)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
    {
        Some(p) if !p.is_empty() => p,
        _ => ".".into(),
    }
}

/// Where an icon pack's JSON or a remote image is read from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resource {
    /// A local file (a `file://` URL or a path).
    File(PathBuf),
    /// An `http:` or `https:` URL, fetched with `curl`.
    Http(String),
}

/// Where `url` is read from: `http(s)://` URLs as they are, `file://` URLs
/// and absolute paths as files, relative paths against `cwd`; `None` for any
/// other scheme.
pub fn resource(url: &str, cwd: &Path) -> Option<Resource> {
    let lower = url.get(..8).unwrap_or(url).to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return Some(Resource::Http(url.to_string()));
    }
    if lower.starts_with("file:") {
        // file:///path, file://localhost/path, file:/path
        let rest = &url[5..];
        let path = match rest.strip_prefix("//") {
            Some(r) => {
                let slash = r.find('/')?;
                let host = &r[..slash];
                if !host.is_empty() && !host.eq_ignore_ascii_case("localhost") {
                    return None;
                }
                &r[slash..]
            }
            None => rest,
        };
        let path = path.split(['?', '#']).next().unwrap_or(path);
        return Some(Resource::File(PathBuf::from(percent_decode(path))));
    }
    // A Windows drive letter is a path, any other `scheme:` is not supported.
    let scheme = url.split_once(':').map(|(s, _)| s);
    if scheme.is_some_and(|s| {
        s.len() > 1
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    }) {
        return None;
    }
    Some(Resource::File(absolutize(cwd, Path::new(url))))
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let Some(v) = s
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Where `--iconPacks <package>` looks for an installed package's
/// `icons.json`, nearest first: `node_modules/<package>/icons.json` in `cwd`
/// and each of its ancestors (Node's resolution from the working directory).
pub fn icon_pack_files(package: &str, cwd: &Path) -> Vec<PathBuf> {
    cwd.ancestors()
        .map(|d| d.join("node_modules").join(package).join("icons.json"))
        .collect()
}

/// The `unpkg.com` URL of a package's `icons.json`, tried when it is not
/// installed.
pub fn icon_pack_cdn_url(package: &str) -> String {
    format!("https://unpkg.com/{package}/icons.json")
}

/// The `prefix` of an Iconify JSON icon set, which `--iconPacks` registers it
/// under.
pub fn icon_pack_prefix(json: &str) -> Option<String> {
    match crate::json::parse(json).ok()?.get("prefix")? {
        crate::json::Json::String(s) => Some(s.clone()),
        _ => None,
    }
}

/// `prefix#url` of `--iconPacksNamesAndUrls`: the name before the first `#`,
/// the URL after it.
pub fn split_icon_pack_arg(arg: &str) -> (String, String) {
    match arg.split_once('#') {
        Some((n, u)) => (n.to_string(), u.to_string()),
        None => (arg.to_string(), String::new()),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn resources_by_scheme() {
        let cwd = Path::new("/work");
        assert_eq!(
            resource("https://x.org/a.json", cwd),
            Some(Resource::Http("https://x.org/a.json".into()))
        );
        assert_eq!(
            resource("file:///tmp/my%20icons.json", cwd),
            Some(Resource::File("/tmp/my icons.json".into()))
        );
        assert_eq!(
            resource("file://localhost/tmp/a.json", cwd),
            Some(Resource::File("/tmp/a.json".into()))
        );
        assert_eq!(resource("file://other/tmp/a.json", cwd), None);
        assert_eq!(
            resource("icons/a.json", cwd),
            Some(Resource::File("/work/icons/a.json".into()))
        );
        assert_eq!(resource("ftp://x/a.json", cwd), None);
        assert_eq!(
            icon_pack_files("@iconify-json/logos", Path::new("/a/b"))[1],
            PathBuf::from("/a/node_modules/@iconify-json/logos/icons.json")
        );
        assert_eq!(
            icon_pack_prefix(r#"{"prefix": "logos", "icons": {}}"#),
            Some("logos".into())
        );
        assert_eq!(icon_pack_prefix(r#"{"icons": {}}"#), None);
        assert_eq!(
            split_icon_pack_arg("az#https://x/#frag"),
            ("az".into(), "https://x/#frag".into())
        );
    }

    use super::*;

    #[test]
    fn defaults() {
        assert_eq!(default_output(Some("a.mmd"), None), "a.mmd.svg");
        assert_eq!(default_output(None, Some(Format::Png)), "out.png");
        assert_eq!(infer_format("x.md", None), Ok(Format::Svg));
        assert_eq!(infer_format("x.pdf", None), Ok(Format::Pdf));
        assert_eq!(infer_format("x.svg", Some(Format::Png)), Ok(Format::Png));
        assert!(infer_format("x.mmd", None).is_err());
    }

    #[test]
    fn markdown_naming() {
        let cwd = Path::new("/work");
        assert_eq!(
            markdown_image_file("docs/out.md", 2, Format::Svg, None, cwd),
            PathBuf::from("docs/out-2.svg")
        );
        assert_eq!(
            markdown_image_file("out.png", 1, Format::Png, None, cwd),
            PathBuf::from("out-1.png")
        );
        let f = markdown_image_file("docs/out.md", 1, Format::Png, Some(Path::new("img")), cwd);
        assert_eq!(f, PathBuf::from("/work/img/out-1.png"));
        assert_eq!(
            markdown_image_url("docs/out.md", &f, cwd),
            "./../img/out-1.png"
        );
        assert_eq!(
            markdown_image_url("docs/out.md", Path::new("docs/out-1.svg"), cwd),
            "./out-1.svg"
        );
    }

    #[test]
    fn dirs() {
        assert_eq!(output_dir("out.svg"), ".");
        assert_eq!(output_dir("a/b/out.svg"), "a/b");
    }
}
