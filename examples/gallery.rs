//! Regenerates the review gallery: renders every `examples/diagrams/*.mmd`
//! to `docs/gallery/<name>.png` (and `.svg`) and writes `docs/GALLERY.md`.
//!
//! `cargo run --release --example gallery`

use mmdc::config::MermaidConfig;
use mmdc::output::{self, EncodeOptions};
use mmdc::render;
use std::fmt::Write as _;
use std::path::Path;
use std::time::Instant;

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src_dir = root.join("examples/diagrams");
    let out_dir = root.join("docs/gallery");
    std::fs::create_dir_all(&out_dir).expect("create docs/gallery");

    let mut files: Vec<_> = std::fs::read_dir(&src_dir)
        .expect("examples/diagrams")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "mmd"))
        .collect();
    files.sort();

    #[cfg(feature = "png")]
    let rast = mmdc::output::png::Rasterizer::new();
    let cfg = MermaidConfig::default();
    let enc = EncodeOptions { scale: 1.5, ..EncodeOptions::default() };

    let mut md = String::from(
        "# Gallery\n\nEvery example in `examples/diagrams/`, rendered by this implementation (no browser).\n\
         Regenerate with `cargo run --release --example gallery`.\n\n",
    );
    let mut failures = 0;
    for path in &files {
        let name = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        let source = std::fs::read_to_string(path).expect("read example");
        let t = Instant::now();
        let rendered = match render::render(&source, &cfg) {
            Ok(r) => r,
            Err(e) => {
                failures += 1;
                eprintln!("{name}: {e}");
                let _ = writeln!(md, "## {name}\n\n**Failed:** `{e}`\n");
                continue;
            }
        };
        let took = t.elapsed();
        std::fs::write(out_dir.join(format!("{name}.svg")), output::encode_svg(&rendered, &enc)).expect("write svg");
        #[cfg(feature = "png")]
        {
            let png = output::encode_png(&rendered, &enc, &rast).expect("png");
            std::fs::write(out_dir.join(format!("{name}.png")), png).expect("write png");
        }
        let vb = rendered.scene.view_box;
        println!("{name:<16} {took:>10.2?}  {:.0}×{:.0}", vb.w, vb.h);
        let _ = writeln!(
            md,
            "## {name}\n\nSource: [`examples/diagrams/{name}.mmd`](../examples/diagrams/{name}.mmd) · rendered in {took:.2?}\n\n\
             <details><summary>source</summary>\n\n```mermaid\n{}\n```\n\n</details>\n\n![{name}](gallery/{name}.png)\n",
            source.trim_end()
        );
    }
    std::fs::write(root.join("docs/GALLERY.md"), md).expect("write GALLERY.md");
    println!("wrote docs/GALLERY.md ({} diagrams, {failures} failures)", files.len());
    if failures > 0 {
        std::process::exit(1);
    }
}
