fn main() {
    println!("cargo:rerun-if-env-changed=THREESTRANDS_GOOGLE_CLIENT_ID");
    println!("cargo:rerun-if-env-changed=THREESTRANDS_GOOGLE_CLIENT_SECRET");
    println!("cargo:rustc-check-cfg=cfg(has_speech_analyzer_shim)");

    if std::env::var("PROFILE").as_deref() == Ok("release")
        && !matches!(
            std::env::var("CARGO_CFG_TARGET_OS").as_deref(),
            Ok("android" | "ios")
        )
    {
        for name in [
            "THREESTRANDS_GOOGLE_CLIENT_ID",
            "THREESTRANDS_GOOGLE_CLIENT_SECRET",
        ] {
            if std::env::var(name)
                .ok()
                .is_none_or(|value| value.trim().is_empty())
            {
                panic!("{name} is required for a release build");
            }
        }
    }

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        compile_speech_analyzer_shim();
    }

    tauri_build::build()
}

fn compile_speech_analyzer_shim() {
    use std::path::PathBuf;
    use std::process::Command;

    println!("cargo:rerun-if-changed=swift/speech_analyzer.swift");

    let Ok(sdk_ver_out) = Command::new("xcrun").args(["--show-sdk-version"]).output() else {
        return;
    };
    if !sdk_ver_out.status.success() {
        return;
    }
    let sdk_ver = String::from_utf8_lossy(&sdk_ver_out.stdout);
    let major: u32 = sdk_ver
        .trim()
        .split('.')
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    if major < 26 {
        return;
    }

    let Ok(sdk_path_out) = Command::new("xcrun").args(["--show-sdk-path"]).output() else {
        return;
    };
    if !sdk_path_out.status.success() {
        return;
    }
    let sdk_path = String::from_utf8_lossy(&sdk_path_out.stdout)
        .trim()
        .to_string();

    let arch = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("x86_64") => "x86_64",
        _ => "arm64",
    };
    let target = format!("{arch}-apple-macosx10.15");
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let lib_path = out_dir.join("libthreestrands_speech_shim.a");

    let status = Command::new("swiftc")
        .args([
            "-emit-library",
            "-static",
            "-parse-as-library",
            "-O",
            "-module-name",
            "ThreeStrandsSpeechShim",
            "-target",
            &target,
            "-sdk",
            &sdk_path,
            "swift/speech_analyzer.swift",
            "-o",
        ])
        .arg(&lib_path)
        .status()
        .expect("running swiftc");

    if !status.success() {
        panic!("swiftc failed compiling swift/speech_analyzer.swift");
    }

    println!("cargo:rustc-cfg=has_speech_analyzer_shim");
    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=threestrands_speech_shim");
    println!("cargo:rustc-link-arg=-Wl,-weak_framework,Speech");
    println!("cargo:rustc-link-lib=framework=AVFoundation");
    println!("cargo:rustc-link-lib=framework=CoreMedia");
    println!("cargo:rustc-link-search=native={sdk_path}/usr/lib/swift");
    if let Ok(swiftc_out) = Command::new("xcrun").args(["--find", "swiftc"]).output() {
        if swiftc_out.status.success() {
            let swiftc_path = PathBuf::from(String::from_utf8_lossy(&swiftc_out.stdout).trim());
            if let Some(lib_dir) = swiftc_path
                .parent()
                .and_then(|p| p.parent())
                .map(|p| p.join("lib/swift/macosx"))
            {
                if lib_dir.is_dir() {
                    println!("cargo:rustc-link-search=native={}", lib_dir.display());
                }
            }
        }
    }
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
}
