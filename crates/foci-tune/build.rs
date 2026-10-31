use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

const STUB: &str =
    "<!-- foci-tune-ui-skipped -->foci-tune web UI skipped (FOCI_TUNE_SKIP_WEB set)\n";

fn main() {
    let manifest = env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    let web = Path::new(&manifest).join("../../web");

    for path in [
        "src",
        "index.html",
        "package.json",
        "package-lock.json",
        "vite.config.ts",
    ] {
        println!("cargo:rerun-if-changed={}", web.join(path).display());
    }
    println!("cargo:rerun-if-env-changed=FOCI_TUNE_SKIP_WEB");

    let dist = web.join("dist");
    if env::var_os("FOCI_TUNE_SKIP_WEB").is_some() {
        let _ = fs::remove_dir_all(&dist);
        fs::create_dir_all(&dist).expect("create web/dist");
        fs::write(dist.join("index.html"), STUB).expect("write the skipped-UI placeholder");
        return;
    }

    if !web.join("node_modules").exists() {
        npm(&web, &["ci"]);
    }
    npm(&web, &["run", "build"]);
}

fn npm(dir: &Path, args: &[&str]) {
    let status = Command::new("npm")
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap_or_else(|e| {
            panic!(
                "cannot run npm ({e}); install Node 22 or set FOCI_TUNE_SKIP_WEB=1 to build \
                 without the web UI"
            )
        });
    assert!(status.success(), "npm {} failed: {status}", args.join(" "));
}
