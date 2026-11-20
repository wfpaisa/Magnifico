use std::fmt::Write as _;
use std::path::Path;

fn main() {
    glib_build_tools::compile_resources(
        &["data"],
        "data/io.github.wfpaisa.Magnifico.gresource.xml",
        "magnifico.gresource",
    );
    embedded_icons();
}

/// Embed line icons without relying on the desktop icon theme.
fn embedded_icons() {
    let mut code = String::from(
        "pub fn embedded_svg(name: &str) -> Option<&'static str> {\n    match name {\n",
    );
    for folder in ["sidebar", "ui"] {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("data/icons")
            .join(folder);
        println!("cargo:rerun-if-changed={}", dir.display());
        let mut files: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "svg"))
            .collect();
        files.sort();
        for path in files {
            println!("cargo:rerun-if-changed={}", path.display());
            let name = format!("{folder}/{}", path.file_stem().unwrap().to_string_lossy());
            writeln!(
                code,
                "        {name:?} => Some(include_str!({:?})),",
                path.display()
            )
            .unwrap();
        }
    }
    code.push_str("        _ => None,\n    }\n}\n");
    let out = Path::new(&std::env::var("OUT_DIR").unwrap()).join("embedded_icons.rs");
    std::fs::write(out, code).unwrap();
}
