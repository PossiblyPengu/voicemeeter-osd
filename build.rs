use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=build.rs");

    // comctl32 v6 gives the controls their modern themed look.
    println!(
        "cargo:rustc-link-arg=/MANIFESTDEPENDENCY:type='win32' \
         name='Microsoft.Windows.Common-Controls' version='6.0.0.0' \
         processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'"
    );

    let Some(rc) = find_rc() else {
        println!("cargo:warning=rc.exe not found; exe will have no embedded icon");
        return;
    };

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let rc_path = out_dir.join("icon.rc");
    let res_path = out_dir.join("icon.res");

    let ico = std::fs::canonicalize("assets/icon.ico").expect("assets/icon.ico missing");
    let ico = ico.to_string_lossy().replace(r"\\?\", "");
    std::fs::write(&rc_path, format!("1 ICON \"{}\"\n", ico.replace('\\', "\\\\"))).unwrap();

    let status = Command::new(&rc)
        .arg("/nologo")
        .arg("/fo")
        .arg(&res_path)
        .arg(&rc_path)
        .status();

    match status {
        Ok(s) if s.success() => println!("cargo:rustc-link-arg={}", res_path.display()),
        _ => println!("cargo:warning=rc.exe failed; exe will have no embedded icon"),
    }
}

fn find_rc() -> Option<PathBuf> {
    let roots = [
        r"C:\Program Files (x86)\Windows Kits\10\bin",
        r"C:\Program Files\Windows Kits\10\bin",
    ];
    let mut best: Option<(String, PathBuf)> = None;
    for root in roots {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let candidate = entry.path().join("x64").join("rc.exe");
            if candidate.exists() {
                let version = entry.file_name().to_string_lossy().to_string();
                if best.as_ref().is_none_or(|(v, _)| version > *v) {
                    best = Some((version, candidate));
                }
            }
        }
    }
    best.map(|(_, path)| path)
}
